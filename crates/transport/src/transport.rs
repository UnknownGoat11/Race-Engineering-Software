//! The transport abstraction ([`TelemetryTransport`], D-004) and its TCP + TLS 1.3 implementation
//! ([`TcpTlsTelemetryTransport`]).
//!
//! Application logic (the shared connection runtime, the [`crate::driver`] and [`crate::engineer`]
//! clients) only sees "connect, then send / receive / close whole frames": a future QUIC transport
//! plugs in behind the same traits. A connection is split into a [`FrameReader`] and a
//! [`FrameWriter`] so that a writer task and the receive loop run concurrently.
//!
//! TCP is a byte stream (PROTOCOL.md §1: one `read()` ≠ one frame): [`StreamFrameReader`] reads
//! into an accumulating buffer and cuts frames with `protocol::FrameDecoder`, which validates magic,
//! major and length from the header before anything is reserved for a frame and verifies the CRC.
//! Any framing error is fatal for the connection.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::BytesMut;
use protocol::{CodecError, Frame, FrameDecoder};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::endpoint::RelayEndpoint;
use crate::error::{ConnectError, ConnectStage, TransportError};
use crate::tls;

/// Bytes requested from the socket per read when no larger frame remainder is known.
const READ_CHUNK_BYTES: usize = 16 * 1024;
/// Default DNS lookup timeout.
pub const DEFAULT_DNS_TIMEOUT: Duration = Duration::from_secs(5);
/// Default timeout of one TCP connection attempt (per resolved address).
pub const DEFAULT_TCP_TIMEOUT: Duration = Duration::from_secs(5);
/// Default TLS handshake timeout.
pub const DEFAULT_TLS_TIMEOUT: Duration = Duration::from_secs(10);

/// Byte counters of one connection, updated by its reader and writer (plaintext frame bytes, not
/// TLS record overhead).
#[derive(Debug, Default)]
pub struct TransportStats {
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

impl TransportStats {
    /// Bytes received so far, including bytes of a frame that is not complete yet.
    pub fn bytes_in(&self) -> u64 {
        self.bytes_in.load(Ordering::Relaxed)
    }

    /// Bytes written so far.
    pub fn bytes_out(&self) -> u64 {
        self.bytes_out.load(Ordering::Relaxed)
    }

    fn add_in(&self, n: usize) {
        self.bytes_in.fetch_add(n as u64, Ordering::Relaxed);
    }

    fn add_out(&self, n: usize) {
        self.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
    }
}

/// Description of an established connection, for diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// Remote address, when the transport has one.
    pub peer: Option<SocketAddr>,
    /// Negotiated TLS version, e.g. `TLSv1_3`.
    pub tls_version: Option<String>,
    /// Negotiated cipher suite.
    pub cipher_suite: Option<String>,
    /// Time from the start of `connect` until the stream was ready for frames.
    pub established_in: Duration,
}

/// A connected frame stream, split into its two directions.
#[derive(Debug)]
pub struct TransportConnection<R, W> {
    pub reader: R,
    pub writer: W,
    /// Byte counters shared by `reader` and `writer`.
    pub stats: Arc<TransportStats>,
    pub info: ConnectionInfo,
}

/// Receiving half of a frame stream.
pub trait FrameReader: Send + 'static {
    /// Receives the next complete, CRC-verified frame. `Ok(None)` means the relay closed the stream
    /// cleanly between frames. **Cancel-safe**: dropping the future loses no data, so it can be
    /// used in `tokio::select!`.
    fn recv(&mut self) -> impl Future<Output = Result<Option<Frame>, TransportError>> + Send;
}

/// Sending half of a frame stream.
pub trait FrameWriter: Send + 'static {
    /// Writes one frame. Not cancel-safe (a dropped future may leave half a frame on the wire):
    /// only a dedicated writer task calls it.
    fn send(&mut self, frame: &Frame) -> impl Future<Output = Result<(), TransportError>> + Send;
    /// Pushes buffered bytes to the network.
    fn flush(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send;
    /// Flushes and closes the sending direction (TLS `close_notify`, TCP FIN).
    fn close(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send;
}

/// A way to reach the relay (TCP + TLS today; QUIC could be added behind the same trait, D-004).
pub trait TelemetryTransport: Send + Sync + 'static {
    type Reader: FrameReader;
    type Writer: FrameWriter;

    /// Opens a connection to `endpoint`, ready to exchange frames (HELLO is the caller's job).
    fn connect(
        &self,
        endpoint: &RelayEndpoint,
    ) -> impl Future<Output = Result<TransportConnection<Self::Reader, Self::Writer>, ConnectError>> + Send;
}

/// [`FrameReader`] over any byte stream.
#[derive(Debug)]
pub struct StreamFrameReader<R> {
    inner: R,
    buf: BytesMut,
    decoder: FrameDecoder,
    stats: Arc<TransportStats>,
}

impl<R> StreamFrameReader<R> {
    /// Reader over `inner`, counting bytes into `stats`.
    pub fn new(inner: R, stats: Arc<TransportStats>) -> Self {
        Self {
            inner,
            buf: BytesMut::with_capacity(READ_CHUNK_BYTES),
            decoder: FrameDecoder::new(),
            stats,
        }
    }
}

impl<R: AsyncRead + Unpin + Send + 'static> FrameReader for StreamFrameReader<R> {
    async fn recv(&mut self) -> Result<Option<Frame>, TransportError> {
        loop {
            if let Some(frame) = self.decoder.decode_frame(&mut self.buf)? {
                return Ok(Some(frame));
            }
            // The header (if buffered) passed validation in decode_frame, so this is bounded by
            // MAX_FRAME_BYTES.
            let needed = self
                .decoder
                .bytes_needed(&self.buf)
                .map_err(CodecError::from)?;
            self.buf.reserve(needed.max(READ_CHUNK_BYTES));
            // read_buf is cancel-safe: if this future is dropped while waiting, nothing was read.
            let n = self.inner.read_buf(&mut self.buf).await?;
            if n == 0 {
                return if self.buf.is_empty() {
                    Ok(None)
                } else {
                    Err(TransportError::TruncatedFrame {
                        buffered: self.buf.len(),
                    })
                };
            }
            self.stats.add_in(n);
        }
    }
}

/// [`FrameWriter`] over any byte stream.
#[derive(Debug)]
pub struct StreamFrameWriter<W> {
    inner: W,
    stats: Arc<TransportStats>,
}

impl<W> StreamFrameWriter<W> {
    /// Writer over `inner`, counting bytes into `stats`.
    pub fn new(inner: W, stats: Arc<TransportStats>) -> Self {
        Self { inner, stats }
    }
}

impl<W: AsyncWrite + Unpin + Send + 'static> FrameWriter for StreamFrameWriter<W> {
    async fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        let bytes = frame.as_bytes();
        self.inner.write_all(bytes).await?;
        self.stats.add_out(bytes.len());
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), TransportError> {
        self.inner.flush().await?;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.inner.shutdown().await?;
        Ok(())
    }
}

/// Splits a connected byte stream (e.g. a TLS stream) into a frame reader and writer sharing one
/// [`TransportStats`].
#[allow(clippy::type_complexity)]
pub fn framed_split<S>(
    stream: S,
) -> (
    StreamFrameReader<ReadHalf<S>>,
    StreamFrameWriter<WriteHalf<S>>,
    Arc<TransportStats>,
)
where
    S: AsyncRead + AsyncWrite,
{
    let stats = Arc::new(TransportStats::default());
    let (read, write) = tokio::io::split(stream);
    (
        StreamFrameReader::new(read, Arc::clone(&stats)),
        StreamFrameWriter::new(write, Arc::clone(&stats)),
        stats,
    )
}

/// Connect timeouts of [`TcpTlsTelemetryTransport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectTimeouts {
    pub dns: Duration,
    pub tcp: Duration,
    pub tls: Duration,
}

impl Default for ConnectTimeouts {
    fn default() -> Self {
        Self {
            dns: DEFAULT_DNS_TIMEOUT,
            tcp: DEFAULT_TCP_TIMEOUT,
            tls: DEFAULT_TLS_TIMEOUT,
        }
    }
}

/// TCP + TLS 1.3 transport (rustls, ALPN `lmure/1`, outbound only: no listening socket, no admin
/// rights, no firewall rule, D-019).
#[derive(Debug, Clone, Default)]
pub struct TcpTlsTelemetryTransport {
    timeouts: ConnectTimeouts,
}

/// Reader type of [`TcpTlsTelemetryTransport`].
pub type TcpTlsReader = StreamFrameReader<ReadHalf<TlsStream<TcpStream>>>;
/// Writer type of [`TcpTlsTelemetryTransport`].
pub type TcpTlsWriter = StreamFrameWriter<WriteHalf<TlsStream<TcpStream>>>;

impl TcpTlsTelemetryTransport {
    /// Transport with the default timeouts.
    pub fn new() -> Self {
        Self::default()
    }

    /// Transport with explicit timeouts.
    pub const fn with_timeouts(timeouts: ConnectTimeouts) -> Self {
        Self { timeouts }
    }

    /// The configured timeouts.
    pub const fn timeouts(&self) -> ConnectTimeouts {
        self.timeouts
    }

    /// Stage 1: resolves the relay host.
    pub async fn resolve(&self, endpoint: &RelayEndpoint) -> Result<Vec<SocketAddr>, ConnectError> {
        let lookup = tokio::net::lookup_host((endpoint.host(), endpoint.port()));
        let addrs: Vec<SocketAddr> = timeout(self.timeouts.dns, lookup)
            .await
            .map_err(|_| ConnectError::Timeout {
                stage: ConnectStage::Dns,
                timeout: self.timeouts.dns,
            })?
            .map_err(|source| ConnectError::Dns {
                host: endpoint.host().to_owned(),
                source,
            })?
            .collect();
        if addrs.is_empty() {
            return Err(ConnectError::NoAddress {
                host: endpoint.host().to_owned(),
            });
        }
        Ok(addrs)
    }

    /// Stage 2: connects to the first reachable address (in resolver order).
    pub async fn connect_tcp(&self, addrs: &[SocketAddr]) -> Result<TcpStream, ConnectError> {
        let mut last_error = None;
        for &addr in addrs {
            match timeout(self.timeouts.tcp, TcpStream::connect(addr)).await {
                Ok(Ok(stream)) => {
                    // Frames are small and latency-critical: never wait for Nagle coalescing.
                    if let Err(e) = stream.set_nodelay(true) {
                        tracing::debug!(component = "transport", error = %e, "TCP_NODELAY not set");
                    }
                    return Ok(stream);
                }
                Ok(Err(source)) => {
                    tracing::debug!(component = "transport", %addr, error = %source, "TCP connect failed");
                    last_error = Some(ConnectError::Tcp { addr, source });
                }
                Err(_) => {
                    tracing::debug!(component = "transport", %addr, "TCP connect timed out");
                    last_error = Some(ConnectError::Timeout {
                        stage: ConnectStage::Tcp,
                        timeout: self.timeouts.tcp,
                    });
                }
            }
        }
        Err(last_error.unwrap_or(ConnectError::NoAddress {
            host: String::new(),
        }))
    }

    /// Stage 3: TLS 1.3 handshake over any byte stream, with certificate validation (webpki roots
    /// plus the development-only extra CA) and the ALPN check.
    pub async fn handshake_tls<S>(
        &self,
        endpoint: &RelayEndpoint,
        stream: S,
    ) -> Result<TlsStream<S>, ConnectError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let config = tls::client_config(endpoint)?;
        let name = tls::server_name(endpoint)?;
        let tls = timeout(
            self.timeouts.tls,
            TlsConnector::from(config).connect(name, stream),
        )
        .await
        .map_err(|_| ConnectError::Timeout {
            stage: ConnectStage::Tls,
            timeout: self.timeouts.tls,
        })?
        .map_err(tls::classify_handshake_error)?;
        if !tls::alpn_ok(tls.get_ref().1.alpn_protocol()) {
            return Err(ConnectError::Alpn);
        }
        Ok(tls)
    }

    /// Version and cipher suite of an established TLS stream, for [`ConnectionInfo`].
    pub fn tls_details<S>(stream: &TlsStream<S>) -> (Option<String>, Option<String>) {
        let conn = stream.get_ref().1;
        (
            conn.protocol_version().map(|v| format!("{v:?}")),
            conn.negotiated_cipher_suite()
                .map(|s| format!("{:?}", s.suite())),
        )
    }
}

impl TelemetryTransport for TcpTlsTelemetryTransport {
    type Reader = TcpTlsReader;
    type Writer = TcpTlsWriter;

    async fn connect(
        &self,
        endpoint: &RelayEndpoint,
    ) -> Result<TransportConnection<TcpTlsReader, TcpTlsWriter>, ConnectError> {
        let started = Instant::now();
        let addrs = self.resolve(endpoint).await?;
        let tcp = self.connect_tcp(&addrs).await?;
        let peer = tcp.peer_addr().ok();
        let tls = self.handshake_tls(endpoint, tcp).await?;
        let (tls_version, cipher_suite) = Self::tls_details(&tls);
        let (reader, writer, stats) = framed_split(tls);
        Ok(TransportConnection {
            reader,
            writer,
            stats,
            info: ConnectionInfo {
                peer,
                tls_version,
                cipher_suite,
                established_in: started.elapsed(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::frame::{FrameMeta, FramePriority, MsgType};

    fn frame(seq: u64, len: usize) -> Frame {
        Frame::build(
            MsgType::TelemetryDelta,
            &FrameMeta::stream(FramePriority::P0, 9, seq, seq),
            &vec![seq as u8; len],
        )
        .expect("frame")
    }

    #[tokio::test]
    async fn frames_survive_arbitrary_chunking_over_a_byte_stream() {
        let (client, server) = tokio::io::duplex(97);
        let (mut reader, _w, stats) = framed_split(client);
        let (_r, mut writer, _s) = framed_split(server);
        let frames: Vec<Frame> = (1..=20).map(|s| frame(s, (s as usize) * 300)).collect();
        let sent = frames.clone();
        let send = tokio::spawn(async move {
            for f in &sent {
                writer.send(f).await.expect("send");
            }
            writer.close().await.expect("close");
        });
        for expected in &frames {
            let got = reader.recv().await.expect("recv").expect("frame");
            assert_eq!(&got, expected);
        }
        assert!(reader.recv().await.expect("clean eof").is_none());
        send.await.expect("sender");
        let total: usize = frames.iter().map(Frame::wire_len).sum();
        assert_eq!(stats.bytes_in(), total as u64);
    }

    #[tokio::test]
    async fn eof_inside_a_frame_is_an_error() {
        let (client, mut server) = tokio::io::duplex(1024);
        let (mut reader, _w, _stats) = framed_split(client);
        let f = frame(1, 100);
        server
            .write_all(&f.as_bytes()[..f.wire_len() - 3])
            .await
            .expect("write");
        drop(server);
        assert!(matches!(
            reader.recv().await,
            Err(TransportError::TruncatedFrame { .. })
        ));
    }

    #[tokio::test]
    async fn garbage_is_a_codec_error() {
        let (client, mut server) = tokio::io::duplex(1024);
        let (mut reader, _w, _stats) = framed_split(client);
        server
            .write_all(b"GET / HTTP/1.1\r\n\r\n")
            .await
            .expect("write");
        assert!(matches!(reader.recv().await, Err(TransportError::Codec(_))));
    }
}
