use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use app_core::bridge::hub::{MAX_HISTORY_BYTES, MAX_SUB_QUEUE_BLOCKS, UiHub};
use app_core::bridge::sink::{SinkClosed, SubscriptionId, UiSink};
use app_core::bridge::status::UiEvent;
use parking_lot::Mutex;
use telemetry_model::channel::ChannelId;
use telemetry_model::codec::RELAY_NS_UNKNOWN;
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::registry::ch;
use telemetry_model::uibatch::{
    self, FLAG_BACKFILL, FLAG_GAP_BEFORE, FLAG_RESET, MAX_BATCH_BYTES, SequenceState,
};
use telemetry_model::value::Value;

#[derive(Default)]
struct MockUiSink {
    batches: Mutex<Vec<(SubscriptionId, Vec<u8>)>>,
    events: Mutex<Vec<UiEvent>>,
    visible: AtomicBool,
    closed: AtomicBool,
}

impl MockUiSink {
    fn new() -> Self {
        Self {
            batches: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            visible: AtomicBool::new(true),
            closed: AtomicBool::new(false),
        }
    }

    fn set_visible(&self, v: bool) {
        self.visible.store(v, Ordering::SeqCst);
    }

    fn set_closed(&self, c: bool) {
        self.closed.store(c, Ordering::SeqCst);
    }

    fn take_batches(&self) -> Vec<(SubscriptionId, Vec<u8>)> {
        std::mem::take(&mut *self.batches.lock())
    }
}

impl UiSink for MockUiSink {
    fn send_batch(&self, sub: SubscriptionId, batch: Vec<u8>) -> Result<(), SinkClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(SinkClosed);
        }
        self.batches.lock().push((sub, batch));
        Ok(())
    }

    fn emit(&self, event: UiEvent) {
        self.events.lock().push(event);
    }

    fn is_visible(&self) -> bool {
        self.visible.load(Ordering::SeqCst)
    }
}

/// Helper to build a valid snapshot TLV block from a TelemetryFrame.
fn encode_snapshot_block(frame: &TelemetryFrame) -> Arc<[u8]> {
    let mut buf = Vec::new();
    frame
        .encode_snapshot(&mut buf, RELAY_NS_UNKNOWN)
        .expect("snapshot encode");
    buf.into()
}

/// Helper to build a valid delta TLV block for modified channels.
fn encode_delta_block(current: &TelemetryFrame, previous: &TelemetryFrame) -> Arc<[u8]> {
    let mut buf = Vec::new();
    current
        .encode_delta(previous, &mut buf, RELAY_NS_UNKNOWN)
        .expect("delta encode");
    buf.into()
}

#[test]
fn backfill_state_equals_live_apply() {
    let hub = UiHub::new();

    let speed_id = ch::SPEED;
    let gear_id = ch::GEAR;
    let throttle_id = ch::THROTTLE;

    let mut oracle = TelemetryFrame::new(0, 1_000_000_000, Some(10.0));
    oracle
        .set_valid(speed_id, Value::F32(50.0))
        .expect("valid speed");
    oracle
        .set_valid(gear_id, Value::I32(3))
        .expect("valid gear");
    oracle
        .set_valid(throttle_id, Value::F32(0.8))
        .expect("valid throttle");

    // 1. Initial snapshot (keyframe)
    let kf0 = encode_snapshot_block(&oracle);
    hub.push_block(kf0, true);

    // 2. Generate 50 delta frames
    let mut prev_frame = oracle.clone();
    for i in 1..=50 {
        let mono_ns: u64 = 1_000_000_000 + (i as u64) * 10_000_000;
        let et = 10.0 + (i as f64) * 0.01;
        oracle.capture_mono_ns = mono_ns;
        oracle.source_et_s = Some(et);
        oracle
            .set_valid(speed_id, Value::F32(50.0 + i as f32 * 0.5))
            .expect("valid speed");
        oracle
            .set_valid(throttle_id, Value::F32((i as f32) / 100.0))
            .expect("valid throttle");

        let delta = encode_delta_block(&oracle, &prev_frame);
        prev_frame = oracle.clone();
        hub.push_block(delta, false);
    }

    // 3. New subscriber joins
    let sink = Arc::new(MockUiSink::new());
    let sub_id = hub.subscribe(Arc::clone(&sink) as Arc<dyn UiSink>);

    let batches = sink.take_batches();
    assert!(!batches.is_empty(), "backfill must deliver batches");
    assert_eq!(batches[0].0, sub_id);

    // 4. Parse all backfill batches and apply all blocks to a reconstructed frame
    let mut recon: Option<TelemetryFrame> = None;
    let mut first_batch = true;

    for (_, batch_bytes) in batches {
        let parsed = uibatch::parse(&batch_bytes).expect("backfill batch must be valid uibatch");
        if first_batch {
            assert!(
                parsed.flags & FLAG_RESET != 0,
                "first backfill batch must have RESET"
            );
            assert!(
                parsed.flags & FLAG_BACKFILL != 0,
                "first backfill batch must have BACKFILL"
            );
            first_batch = false;
        } else {
            assert!(
                parsed.flags & FLAG_BACKFILL != 0,
                "subsequent backfill batch must have BACKFILL"
            );
        }

        for block_slice in parsed.block_slices(&batch_bytes) {
            match recon {
                None => {
                    let (frame, _) = TelemetryFrame::from_snapshot(0, block_slice)
                        .expect("first block is keyframe snapshot");
                    recon = Some(frame);
                }
                Some(ref mut current_frame) => {
                    let (updated, _) = current_frame
                        .apply_tlv(0, block_slice)
                        .expect("block must apply cleanly");
                    *current_frame = updated;
                }
            }
        }
    }

    let recon = recon.expect("must have reconstructed frame");

    // 5. Assert reconstructed frame equals oracle state byte-for-byte / value-for-value
    assert_eq!(recon.f64(speed_id), oracle.f64(speed_id));
    assert_eq!(recon.f64(throttle_id), oracle.f64(throttle_id));
    assert_eq!(recon.get(gear_id), oracle.get(gear_id));
    assert_eq!(recon.capture_mono_ns, oracle.capture_mono_ns);
    assert_eq!(recon.source_et_s, oracle.source_et_s);
}

#[test]
fn queue_overflow_emits_gap_before() {
    let hub = UiHub::new();
    let sink = Arc::new(MockUiSink::new());
    let sub_id = hub.subscribe(Arc::clone(&sink) as Arc<dyn UiSink>);
    let _ = sink.take_batches(); // clear backfill

    let mut frame = TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
    frame
        .set_valid(ChannelId(1), Value::F32(10.0))
        .expect("valid channel 1");
    let block = encode_snapshot_block(&frame);

    // Push more than MAX_SUB_QUEUE_BLOCKS (1024) without flushing
    for _ in 0..(MAX_SUB_QUEUE_BLOCKS + 100) {
        hub.push_block(Arc::clone(&block), false);
    }

    assert!(
        hub.total_ui_dropped() >= 100,
        "overflow must increment total_ui_dropped: {}",
        hub.total_ui_dropped()
    );
    assert!(
        hub.subscription_dropped(sub_id).unwrap_or(0) >= 100,
        "overflow must increment sub drops"
    );

    // Flush and verify batch has FLAG_GAP_BEFORE
    hub.flush_subscription(sub_id).expect("flush subscription");
    let batches = sink.take_batches();
    assert!(!batches.is_empty());

    let parsed = uibatch::parse(&batches[0].1).expect("batch parse");
    assert!(
        parsed.flags & FLAG_GAP_BEFORE != 0,
        "overflow batch must have FLAG_GAP_BEFORE set (flags: {:#x})",
        parsed.flags
    );
}

#[test]
fn batch_seq_monotonic_jump_and_wrap() {
    let hub = UiHub::new();
    let sink = Arc::new(MockUiSink::new());
    let sub_id = hub.subscribe(Arc::clone(&sink) as Arc<dyn UiSink>);
    let initial_batches = sink.take_batches();

    let mut seq_state = SequenceState::default();
    let mut expected_seq = 0u32;

    for (_, b) in initial_batches {
        let p = uibatch::parse(&b).expect("uibatch parse");
        assert_eq!(p.batch_seq, expected_seq);
        let gap = seq_state.observe(p.flags, p.batch_seq);
        assert!(!gap, "initial backfill has no gap");
        expected_seq = expected_seq.wrapping_add(1);
    }

    let mut frame = TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
    frame
        .set_valid(ChannelId(1), Value::F32(12.0))
        .expect("valid channel 1");
    let block = encode_snapshot_block(&frame);

    // Push and flush 10 separate batches
    for _ in 0..10 {
        hub.push_block(Arc::clone(&block), false);
        hub.flush_subscription(sub_id).expect("flush subscription");
        let batches = sink.take_batches();
        for (_, b) in batches {
            let p = uibatch::parse(&b).expect("uibatch parse");
            assert_eq!(p.batch_seq, expected_seq);
            let gap = seq_state.observe(p.flags, p.batch_seq);
            assert!(!gap);
            expected_seq = expected_seq.wrapping_add(1);
        }
    }

    // Test sequence gap detection in SequenceState directly:
    // If sequence jumps by 2 without RESET, observe detects a gap:
    assert!(seq_state.observe(0, expected_seq.wrapping_add(2)));

    // A RESET batch starts a fresh sequence without counting as a jump:
    assert!(!seq_state.observe(FLAG_RESET, 500));
}

#[test]
fn batches_stay_under_4mib() {
    let hub = UiHub::new();

    // Create large blocks (each 50 KB)
    let mut dummy = vec![0u8; 50_000];
    dummy[2..10].copy_from_slice(&1_000_000_000u64.to_le_bytes());
    let large_block: Arc<[u8]> = dummy.into();

    // Push 120 blocks (~6 MB of total history, exceeding the 4 MiB batch limit)
    hub.push_block(Arc::clone(&large_block), true);
    for _ in 1..120 {
        hub.push_block(Arc::clone(&large_block), false);
    }

    let sink = Arc::new(MockUiSink::new());
    let _sub_id = hub.subscribe(Arc::clone(&sink) as Arc<dyn UiSink>);

    let batches = sink.take_batches();
    assert!(
        batches.len() >= 2,
        "6 MB history must split into at least 2 batches: got {}",
        batches.len()
    );

    for (i, (_, batch)) in batches.iter().enumerate() {
        assert!(
            batch.len() <= MAX_BATCH_BYTES,
            "batch {} exceeds MAX_BATCH_BYTES: {} > {}",
            i,
            batch.len(),
            MAX_BATCH_BYTES
        );
        let parsed = uibatch::parse(batch).expect("batch must be valid uibatch");
        if i == 0 {
            assert!(parsed.flags & FLAG_RESET != 0);
        }
        assert!(parsed.flags & FLAG_BACKFILL != 0);
    }
}

#[test]
fn visibility_pause_and_reprime() {
    let hub = UiHub::new();
    let sink = Arc::new(MockUiSink::new());
    let sub_id = hub.subscribe(Arc::clone(&sink) as Arc<dyn UiSink>);
    let _ = sink.take_batches(); // clear initial backfill

    let mut frame = TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
    frame
        .set_valid(ChannelId(1), Value::F32(1.0))
        .expect("valid channel 1");
    let b1 = encode_snapshot_block(&frame);
    hub.push_block(b1, true);
    hub.flush_subscription(sub_id).expect("flush subscription");

    let batches1 = sink.take_batches();
    assert_eq!(batches1.len(), 1);

    // 1. Hide the sink (visibility = false)
    sink.set_visible(false);

    // Push updates while hidden
    frame
        .set_valid(ChannelId(1), Value::F32(2.0))
        .expect("valid channel 1");
    let b2 = encode_snapshot_block(&frame);
    hub.push_block(b2, false);
    hub.flush_subscription(sub_id).expect("flush subscription");

    let batches_hidden = sink.take_batches();
    assert!(
        batches_hidden.is_empty(),
        "no batches should be delivered when invisible"
    );

    // 2. Make sink visible again
    sink.set_visible(true);
    frame
        .set_valid(ChannelId(1), Value::F32(3.0))
        .expect("valid channel 1");
    let b3 = encode_snapshot_block(&frame);
    hub.push_block(b3, false);
    hub.flush_subscription(sub_id).expect("flush subscription");

    // 3. Must have received a re-prime batch with RESET | BACKFILL
    let reprime_batches = sink.take_batches();
    assert!(!reprime_batches.is_empty());

    let parsed = uibatch::parse(&reprime_batches[0].1).expect("uibatch parse");
    assert!(
        parsed.flags & FLAG_RESET != 0,
        "re-prime batch must have RESET"
    );
    assert!(
        parsed.flags & FLAG_BACKFILL != 0,
        "re-prime batch must have BACKFILL"
    );
}

#[test]
fn history_hole_after_gap_emits_gap_before_in_backfill() {
    let hub = UiHub::new();

    let mut frame = TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
    frame
        .set_valid(ChannelId(1), Value::F32(100.0))
        .expect("valid channel 1");
    let kf = encode_snapshot_block(&frame);
    hub.push_block(kf, true);

    // Push regular delta
    frame
        .set_valid(ChannelId(1), Value::F32(101.0))
        .expect("valid channel 1");
    let d1 = encode_snapshot_block(&frame);
    hub.push_block(d1, false);

    // Push delta following a transport gap
    frame
        .set_valid(ChannelId(1), Value::F32(105.0))
        .expect("valid channel 1");
    let d2 = encode_snapshot_block(&frame);
    hub.push_block_with_gap(d2, false, true);

    // Subscribe now: backfill must emit pre-gap blocks, then post-gap batch with GAP_BEFORE
    let sink = Arc::new(MockUiSink::new());
    let _sub_id = hub.subscribe(Arc::clone(&sink) as Arc<dyn UiSink>);

    let batches = sink.take_batches();
    assert!(
        batches.len() >= 2,
        "gap in history must split backfill: got {}",
        batches.len()
    );

    let p0 = uibatch::parse(&batches[0].1).expect("uibatch parse");
    assert_eq!(
        p0.flags & (FLAG_RESET | FLAG_BACKFILL),
        FLAG_RESET | FLAG_BACKFILL
    );
    assert_eq!(p0.flags & FLAG_GAP_BEFORE, 0);

    let p1 = uibatch::parse(&batches[1].1).expect("uibatch parse");
    assert!(
        p1.flags & FLAG_GAP_BEFORE != 0,
        "batch after gap must have FLAG_GAP_BEFORE: flags {:#x}",
        p1.flags
    );
    assert!(p1.flags & FLAG_BACKFILL != 0);
}

#[test]
fn memory_bound_under_10_minute_fuzz() {
    let hub = UiHub::new();

    let mut frame = TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
    frame
        .set_valid(ChannelId(1), Value::F32(1.0))
        .expect("valid channel 1");
    let block = encode_snapshot_block(&frame);

    // 10 minutes at 100 Hz = 60,000 blocks
    // Keyframes every 100 blocks (1 s)
    for i in 0..60_000 {
        let mono_ns: u64 = 1_000_000_000 + (i as u64) * 10_000_000;
        let is_kf = i % 100 == 0;

        let mut b = block.to_vec();
        // Overwrite capture_mono_ns in header (offsets 2..10)
        b[2..10].copy_from_slice(&mono_ns.to_le_bytes());

        hub.push_block(b.into(), is_kf);

        // Every 5,000 blocks check that memory does not exceed hard cap
        if i % 5_000 == 0 {
            let (_, bytes) = hub.history_stats();
            assert!(
                bytes <= MAX_HISTORY_BYTES,
                "history bytes {} exceeded MAX_HISTORY_BYTES {}",
                bytes,
                MAX_HISTORY_BYTES
            );
        }
    }

    let (blocks, bytes) = hub.history_stats();
    assert!(bytes <= MAX_HISTORY_BYTES);
    // At 100 Hz with 60 s retention, retained blocks should be around 6,000 to 6,100
    assert!(
        (5_900..=6_200).contains(&blocks),
        "retained blocks {} outside expected 60s window range",
        blocks
    );
}

#[test]
fn dead_subscription_pruned_on_sink_closed() {
    let hub = UiHub::new();
    let sink = Arc::new(MockUiSink::new());
    let sub_id = hub.subscribe(Arc::clone(&sink) as Arc<dyn UiSink>);

    assert_eq!(hub.subscription_count(), 1);

    // Close sink
    sink.set_closed(true);

    let mut frame = TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
    frame
        .set_valid(ChannelId(1), Value::F32(1.0))
        .expect("valid channel 1");
    let block = encode_snapshot_block(&frame);
    hub.push_block(block, true);

    // Flushing should return Err(SinkClosed) and prune the subscription
    let res = hub.flush_subscription(sub_id);
    assert_eq!(res, Err(SinkClosed));
    assert_eq!(
        hub.subscription_count(),
        0,
        "subscription must be pruned when sink closes"
    );
}

#[test]
fn pacer_flushes_and_emits_status_cleanly() {
    use app_core::bridge::pacer::{PacerConfig, UiBridgePacer};
    use std::sync::atomic::AtomicUsize;
    use std::thread;
    use std::time::Duration;

    let hub = Arc::new(UiHub::new());
    let sink = Arc::new(MockUiSink::new());
    let _sub_id = hub.subscribe(Arc::clone(&sink) as Arc<dyn UiSink>);
    let _ = sink.take_batches(); // clear initial backfill

    let status_calls = Arc::new(AtomicUsize::new(0));
    let status_calls_clone = Arc::clone(&status_calls);

    let config = PacerConfig {
        flush_hz: 120,
        status_hz: 20,
    };

    let mut pacer = UiBridgePacer::spawn(
        Arc::clone(&hub),
        config,
        Some(move || {
            status_calls_clone.fetch_add(1, Ordering::SeqCst);
        }),
    );

    let mut frame = TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
    frame
        .set_valid(ChannelId(1), Value::F32(42.0))
        .expect("valid channel 1");
    let block = encode_snapshot_block(&frame);

    for _ in 0..5 {
        hub.push_block(Arc::clone(&block), false);
        thread::sleep(Duration::from_millis(25));
    }

    thread::sleep(Duration::from_millis(100));

    let batches = sink.take_batches();
    assert!(
        !batches.is_empty(),
        "pacer must have flushed batches to sink"
    );

    let calls = status_calls.load(Ordering::SeqCst);
    assert!(calls >= 2, "status emitter must have fired, got {}", calls);

    pacer.stop();
}
