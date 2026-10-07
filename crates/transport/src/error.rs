//! Error types of the byte-stream layer: establishing a connection ([`ConnectError`]), TLS
//! configuration ([`TlsConfigError`]) and an established frame stream ([`TransportError`]).
//! Protocol-level failures (handshake, session) live in [`crate::handshake`]; credential storage in
//! [`crate::credentials`].

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use protocol::CodecError;
use protocol::version::Environment;
use thiserror::Error;

/// Step of establishing a connection (also the first stages of the connectivity test).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectStage {
    /// Resolving the relay host name.
    Dns,
    /// Opening the TCP connection.
    Tcp,
    /// TLS 1.3 handshake including certificate validation and ALPN.
    Tls,
}

impl std::fmt::Display for ConnectStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ConnectStage::Dns => "DNS lookup",
            ConnectStage::Tcp => "TCP connect",
            ConnectStage::Tls => "TLS handshake",
        })
    }
}

/// Building the TLS client configuration failed. Never retryable without a configuration change.
#[derive(Debug, Error)]
pub enum TlsConfigError {
    #[error(
        "an extra trusted CA is only allowed for the development environment (endpoint is {environment:?})"
    )]
    ExtraCaOutsideDevelopment { environment: Environment },
    #[error("cannot read the extra CA file {path}")]
    ExtraCaRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the extra CA PEM data is invalid")]
    ExtraCaPem {
        #[source]
        source: rustls::pki_types::pem::Error,
    },
    #[error("the extra CA PEM data contains no certificate")]
    ExtraCaEmpty,
    #[error("an extra CA certificate was rejected")]
    ExtraCaRejected {
        #[source]
        source: rustls::Error,
    },
    #[error("the TLS provider does not support TLS 1.3")]
    Tls13Unavailable {
        #[source]
        source: rustls::Error,
    },
    #[error("{host:?} is not a valid TLS server name")]
    ServerName { host: String },
}

/// Establishing the byte stream to the relay failed (DNS, TCP or TLS).
#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("TLS configuration error")]
    TlsConfig(#[from] TlsConfigError),
    #[error("DNS lookup of {host} failed")]
    Dns {
        host: String,
        #[source]
        source: io::Error,
    },
    #[error("DNS lookup of {host} returned no address")]
    NoAddress { host: String },
    #[error("TCP connection to {addr} failed")]
    Tcp {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },
    #[error("{stage} timed out after {timeout:?}")]
    Timeout {
        stage: ConnectStage,
        timeout: Duration,
    },
    /// The relay's certificate failed validation (unknown issuer, expired, wrong name, ...). The
    /// connection was refused before any byte of the protocol was sent.
    #[error("the relay's TLS certificate is not trusted")]
    UntrustedCertificate {
        #[source]
        source: rustls::Error,
    },
    #[error("TLS handshake with the relay failed")]
    Tls {
        #[source]
        source: io::Error,
    },
    /// TLS succeeded but the server did not select ALPN `lmure/1`: it is not an lmure relay (or a
    /// TLS-intercepting proxy is in the way).
    #[error("the server did not negotiate ALPN lmure/1")]
    Alpn,
}

impl ConnectError {
    /// The stage this error belongs to (configuration errors count as TLS).
    pub const fn stage(&self) -> ConnectStage {
        match self {
            ConnectError::Dns { .. } | ConnectError::NoAddress { .. } => ConnectStage::Dns,
            ConnectError::Tcp { .. } => ConnectStage::Tcp,
            ConnectError::Timeout { stage, .. } => *stage,
            ConnectError::TlsConfig(_)
            | ConnectError::UntrustedCertificate { .. }
            | ConnectError::Tls { .. }
            | ConnectError::Alpn => ConnectStage::Tls,
        }
    }

    /// Whether retrying later can succeed without a configuration change. Certificate and ALPN
    /// failures are retried too (a captive portal or an expired relay certificate can be fixed
    /// without touching this client), but surfaced prominently; configuration errors are not.
    pub const fn is_retryable(&self) -> bool {
        !matches!(self, ConnectError::TlsConfig(_))
    }
}

/// An established frame stream failed.
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("I/O error on the relay connection")]
    Io(#[from] io::Error),
    /// The relay sent bytes that are not a valid frame stream (fatal for the connection).
    #[error("invalid frame stream from the relay")]
    Codec(#[from] CodecError),
    #[error("writing a frame took longer than {timeout:?}")]
    WriteTimeout { timeout: Duration },
    #[error("the relay closed the connection in the middle of a frame ({buffered} bytes buffered)")]
    TruncatedFrame { buffered: usize },
}
