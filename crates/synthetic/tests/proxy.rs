//! The fault proxy on real localhost sockets: loss rates, whole-frame integrity, latency, blackouts,
//! connection drops, restarts, framing errors and opaque mode.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use protocol::codec::FrameDecoder;
use protocol::frame::{Frame, FrameMeta, FramePriority, MsgType};
use protocol::sequence::{Arrival, SequenceTracker};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use synthetic::{
    BlackoutPolicy, FaultConfig, FaultProxy, FaultProxyConfig, Jitter, LossPreset, ProxyMode,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

const SESSION: u64 = 7;
const WAIT: Duration = Duration::from_secs(10);
const SINK_QUEUE: usize = 100_000;

fn localhost() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

fn telemetry(seq: u64, rng: &mut ChaCha8Rng) -> Frame {
    let len = rng.random_range(1..2_000);
    let payload: Vec<u8> = (0..len).map(|_| rng.random()).collect();
    let meta = FrameMeta::stream(FramePriority::P0, SESSION, seq, seq * 10_000_000);
    Frame::build(MsgType::TelemetryDelta, &meta, &payload).expect("telemetry frame")
}

fn ping(ts: u64) -> Frame {
    Frame::build(MsgType::Ping, &FrameMeta::control(SESSION, ts), &[]).expect("ping frame")
}

/// What the sink received: (connection index, frame, arrival time).
type Received = (usize, Frame, Instant);

/// A server that decodes every frame it receives (any corruption fails the CRC check and panics).
async fn sink() -> (SocketAddr, mpsc::Receiver<Received>) {
    let listener = TcpListener::bind(localhost()).await.expect("bind sink");
    let addr = listener.local_addr().expect("sink addr");
    let (tx, rx) = mpsc::channel(SINK_QUEUE);
    tokio::spawn(async move {
        let mut index = 0;
        while let Ok((mut stream, _)) = listener.accept().await {
            let tx = tx.clone();
            let conn = index;
            index += 1;
            tokio::spawn(async move {
                let mut decoder = FrameDecoder::new();
                let mut buf = BytesMut::new();
                loop {
                    while let Some(frame) = decoder.decode_frame(&mut buf).expect("intact frames") {
                        if tx.send((conn, frame, Instant::now())).await.is_err() {
                            return;
                        }
                    }
                    buf.reserve(4_096);
                    match stream.read_buf(&mut buf).await {
                        Ok(0) | Err(_) => {
                            assert!(buf.is_empty(), "partial frame forwarded");
                            return;
                        }
                        Ok(_) => {}
                    }
                }
            });
        }
    });
    (addr, rx)
}

async fn proxy(upstream: SocketAddr, to_upstream: FaultConfig, seed: u64) -> FaultProxy {
    let mut cfg = FaultProxyConfig::new(localhost(), upstream);
    cfg.to_upstream = to_upstream;
    cfg.seed = seed;
    FaultProxy::start(cfg).await.expect("proxy")
}

/// Writes `bytes` in random-sized chunks that ignore frame boundaries.
async fn write_chunked(stream: &mut TcpStream, bytes: &[u8], rng: &mut ChaCha8Rng) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let n = rng.random_range(1..=3_000).min(rest.len());
        stream.write_all(&rest[..n]).await.expect("write");
        rest = &rest[n..];
    }
}

async fn collect(rx: &mut mpsc::Receiver<Received>, until_pings: usize) -> Vec<Received> {
    let mut out = Vec::new();
    let mut pings = 0;
    while pings < until_pings {
        let item = timeout(WAIT, rx.recv())
            .await
            .expect("sink timed out")
            .expect("sink open");
        if item.1.msg_type() == Some(MsgType::Ping) {
            pings += 1;
        }
        out.push(item);
    }
    out
}

#[tokio::test]
async fn loss_presets_drop_whole_frames_within_tolerance() {
    const FRAMES: u64 = 3_000;
    const PING_EVERY: u64 = 100;
    for (k, preset) in LossPreset::ALL.into_iter().enumerate() {
        let (upstream, mut rx) = sink().await;
        let faults = FaultConfig::new(preset, Duration::ZERO, Jitter::Low);
        let proxy = proxy(upstream, faults, 1_000 + k as u64).await;
        let mut rng = ChaCha8Rng::seed_from_u64(k as u64);
        let mut sent = Vec::new();
        let mut wire = Vec::new();
        for seq in 1..=FRAMES {
            let f = telemetry(seq, &mut rng);
            wire.extend_from_slice(f.as_bytes());
            sent.push(Bytes::copy_from_slice(f.as_bytes()));
            if seq % PING_EVERY == 0 {
                wire.extend_from_slice(ping(seq).as_bytes());
            }
        }
        let mut client = TcpStream::connect(proxy.local_addr())
            .await
            .expect("connect");
        write_chunked(&mut client, &wire, &mut rng).await;
        let received = collect(&mut rx, (FRAMES / PING_EVERY) as usize).await;

        let mut tracker = SequenceTracker::new();
        let mut missing = 0;
        let mut delivered = 0;
        for (_, frame, _) in &received {
            if frame.msg_type() != Some(MsgType::TelemetryDelta) {
                continue;
            }
            delivered += 1;
            let seq = frame.header().sequence;
            // Byte-identical to what the client sent.
            assert_eq!(frame.as_bytes(), &sent[(seq - 1) as usize][..]);
            match tracker.observe(seq) {
                Arrival::Gap { missing: m } => missing += m,
                Arrival::First | Arrival::InOrder => {}
                other => panic!("unexpected arrival {other:?}"),
            }
        }
        // Frames after the last delivered one are not seen as a gap; the last frame precedes a ping, so the
        // tail is complete once all pings arrived (pings are never dropped).
        let last = received
            .iter()
            .filter(|(_, f, _)| f.msg_type() == Some(MsgType::TelemetryDelta))
            .map(|(_, f, _)| f.header().sequence)
            .max()
            .unwrap_or(0);
        let tail = FRAMES - last;
        let dropped = FRAMES - delivered;
        let first_missing = received
            .iter()
            .find(|(_, f, _)| f.msg_type() == Some(MsgType::TelemetryDelta))
            .map_or(0, |(_, f, _)| f.header().sequence - 1);
        assert_eq!(missing + tail + first_missing, dropped, "{preset:?}");
        let stats = proxy.stats();
        assert_eq!(stats.units_dropped_loss, dropped, "{preset:?}");
        assert_eq!(stats.framing_errors, 0);
        let p = preset.probability();
        let rate = dropped as f64 / FRAMES as f64;
        let tolerance = (5.0 * (p * (1.0 - p) / FRAMES as f64).sqrt()).max(0.004);
        assert!((rate - p).abs() <= tolerance, "{preset:?}: measured {rate}");
        proxy.shutdown().await;
    }
}

#[tokio::test]
async fn latency_delays_every_frame() {
    let (upstream, mut rx) = sink().await;
    let latency = Duration::from_millis(100);
    let proxy = proxy(
        upstream,
        FaultConfig::new(LossPreset::None, latency, Jitter::None),
        1,
    )
    .await;
    let mut client = TcpStream::connect(proxy.local_addr())
        .await
        .expect("connect");
    for i in 0..5 {
        let sent_at = Instant::now();
        client.write_all(ping(i).as_bytes()).await.expect("write");
        let (_, _, at) = timeout(WAIT, rx.recv())
            .await
            .expect("timely")
            .expect("open");
        let delay = at - sent_at;
        assert!(delay >= latency, "{delay:?}");
        assert!(delay < latency * 4, "{delay:?}");
    }
    proxy.shutdown().await;
}

#[tokio::test]
async fn blackout_drops_telemetry_and_holds_reliable_frames() {
    let (upstream, mut rx) = sink().await;
    let faults = FaultConfig::new(LossPreset::None, Duration::ZERO, Jitter::None)
        .with_blackout_policy(BlackoutPolicy::Drop);
    let proxy = proxy(upstream, faults, 2).await;
    let mut client = TcpStream::connect(proxy.local_addr())
        .await
        .expect("connect");
    let mut rng = ChaCha8Rng::seed_from_u64(3);
    client
        .write_all(telemetry(1, &mut rng).as_bytes())
        .await
        .expect("write");
    let first = timeout(WAIT, rx.recv())
        .await
        .expect("timely")
        .expect("open");
    assert_eq!(first.1.header().sequence, 1);

    let blackout = Duration::from_millis(400);
    proxy.blackout(blackout);
    let started = Instant::now();
    for seq in 2..=11 {
        client
            .write_all(telemetry(seq, &mut rng).as_bytes())
            .await
            .expect("write");
    }
    client.write_all(ping(99).as_bytes()).await.expect("write");
    let held = timeout(WAIT, rx.recv())
        .await
        .expect("timely")
        .expect("open");
    assert_eq!(
        held.1.msg_type(),
        Some(MsgType::Ping),
        "telemetry during the blackout is dropped"
    );
    assert!(held.2 - started >= blackout - Duration::from_millis(50));
    client
        .write_all(telemetry(12, &mut rng).as_bytes())
        .await
        .expect("write");
    let after = timeout(WAIT, rx.recv())
        .await
        .expect("timely")
        .expect("open");
    assert_eq!(after.1.header().sequence, 12);
    assert_eq!(proxy.stats().units_dropped_blackout, 10);
    proxy.shutdown().await;
}

async fn expect_closed(stream: &mut TcpStream) {
    let mut byte = [0_u8; 1];
    let read = timeout(WAIT, stream.read(&mut byte))
        .await
        .expect("closed in time");
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
}

#[tokio::test]
async fn connection_drop_restart_and_lifetime() {
    let (upstream, mut rx) = sink().await;
    let mut cfg = FaultProxyConfig::new(localhost(), upstream);
    cfg.drop_after = Some(Duration::from_secs(3));
    let proxy = FaultProxy::start(cfg).await.expect("proxy");

    let mut a = TcpStream::connect(proxy.local_addr())
        .await
        .expect("connect");
    a.write_all(ping(1).as_bytes()).await.expect("write");
    assert_eq!(
        timeout(WAIT, rx.recv())
            .await
            .expect("timely")
            .expect("open")
            .0,
        0
    );
    proxy.drop_connections();
    expect_closed(&mut a).await;

    let downtime = Duration::from_millis(500);
    proxy.restart(downtime);
    let mut refused = TcpStream::connect(proxy.local_addr())
        .await
        .expect("tcp accept");
    expect_closed(&mut refused).await;
    tokio::time::sleep(downtime).await;
    let mut b = TcpStream::connect(proxy.local_addr())
        .await
        .expect("connect");
    b.write_all(ping(2).as_bytes()).await.expect("write");
    let (conn, frame, _) = timeout(WAIT, rx.recv())
        .await
        .expect("timely")
        .expect("open");
    assert_eq!((conn, frame.header().source_ts_ns), (1, 2));
    // The fixed lifetime drops the connection too.
    let lived = Instant::now();
    expect_closed(&mut b).await;
    assert!(lived.elapsed() <= Duration::from_secs(3) + Duration::from_millis(500));

    let stats = proxy.stats();
    assert_eq!(stats.connections_accepted, 2);
    assert_eq!(stats.connections_refused, 1);
    assert_eq!(stats.connections_dropped, 2);
    proxy.shutdown().await;
}

#[tokio::test]
async fn framing_errors_close_the_connection_without_forwarding() {
    let (upstream, mut rx) = sink().await;
    let proxy = proxy(upstream, FaultConfig::default(), 4).await;
    let mut client = TcpStream::connect(proxy.local_addr())
        .await
        .expect("connect");
    let mut wire = ping(5).as_bytes().to_vec();
    wire.extend_from_slice(b"GARBAGE THAT IS NOT A FRAME HEADER AT ALL......");
    client.write_all(&wire).await.expect("write");
    let (_, frame, _) = timeout(WAIT, rx.recv())
        .await
        .expect("timely")
        .expect("open");
    assert_eq!(frame.header().source_ts_ns, 5);
    expect_closed(&mut client).await;
    assert_eq!(proxy.stats().framing_errors, 1);
    assert!(
        timeout(Duration::from_millis(200), rx.recv())
            .await
            .is_err(),
        "nothing after the garbage is forwarded"
    );
    proxy.shutdown().await;
}

#[tokio::test]
async fn opaque_mode_forwards_bytes_unchanged_and_rejects_loss() {
    let mut cfg = FaultProxyConfig::new(localhost(), localhost());
    cfg.mode = ProxyMode::Opaque;
    cfg.to_upstream = FaultConfig::new(LossPreset::OnePercent, Duration::ZERO, Jitter::None);
    assert!(FaultProxy::start(cfg).await.is_err());

    let listener = TcpListener::bind(localhost()).await.expect("bind");
    let upstream = listener.local_addr().expect("addr");
    let mut cfg = FaultProxyConfig::new(localhost(), upstream);
    cfg.mode = ProxyMode::Opaque;
    cfg.to_upstream = FaultConfig::new(LossPreset::None, Duration::from_millis(20), Jitter::Medium);
    let proxy = FaultProxy::start(cfg).await.expect("proxy");
    let mut rng = ChaCha8Rng::seed_from_u64(9);
    let data: Vec<u8> = (0..200_000).map(|_| rng.random()).collect();
    let mut client = TcpStream::connect(proxy.local_addr())
        .await
        .expect("connect");
    let (mut server, _) = listener.accept().await.expect("accept");
    write_chunked(&mut client, &data, &mut rng).await;
    client.shutdown().await.expect("half-close");
    let mut got = Vec::new();
    timeout(WAIT, server.read_to_end(&mut got))
        .await
        .expect("timely")
        .expect("read");
    assert_eq!(got, data);
    assert!(
        proxy
            .set_faults(
                FaultConfig::new(LossPreset::FivePercent, Duration::ZERO, Jitter::None),
                FaultConfig::default()
            )
            .is_err()
    );
    proxy.shutdown().await;
}
