//! [`RelayEndpoint`]: where the relay is and which deployment environment it must be.
//!
//! No relay address is hard-coded anywhere in the transport: every connection is made to an
//! endpoint the application loaded from its settings (D-020). The endpoint also carries:
//! * the **environment** the client is configured for: a relay announcing another environment in
//!   `ServerHello.env` is refused before any credential is sent (PROTOCOL.md §6);
//! * an optional extra trusted CA, accepted **only** for [`Environment::Development`] (D-016:
//!   self-hosted development relays use a locally generated CA). Certificate validation itself can
//!   never be disabled (ARCHITECTURE.md §7).

use std::fmt;
use std::path::PathBuf;

pub use protocol::version::Environment;
use rustls::pki_types::ServerName;
use thiserror::Error;

/// Separator between host and port in `host:port`.
const PORT_SEPARATOR: char = ':';
/// Brackets around an IPv6 literal in `[addr]:port`.
const IPV6_OPEN: char = '[';
const IPV6_CLOSE: char = ']';
/// URL scheme marker; endpoints are plain `host[:port]`, never URLs.
const SCHEME_MARKER: &str = "://";

/// An extra CA certificate trusted in addition to the public web PKI roots. Development only.
#[derive(Clone, PartialEq, Eq)]
pub enum ExtraCa {
    /// PEM file with one or more CA certificates (e.g. written by `lmu-relay dev-cert`).
    PemFile(PathBuf),
    /// PEM text with one or more CA certificates.
    Pem(Vec<u8>),
}

impl fmt::Debug for ExtraCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtraCa::PemFile(path) => f.debug_tuple("PemFile").field(path).finish(),
            ExtraCa::Pem(bytes) => write!(f, "Pem({} bytes)", bytes.len()),
        }
    }
}

/// Why an endpoint is not usable.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EndpointError {
    #[error("the relay host is empty")]
    EmptyHost,
    #[error("the relay host must be a host name or IP address, not a URL")]
    UrlNotHost,
    #[error("the relay host {host:?} is not a valid DNS name or IP address")]
    InvalidHost { host: String },
    #[error("the relay port must be between 1 and 65535")]
    InvalidPort,
    #[error(
        "an extra trusted CA is only allowed for the development environment (endpoint is {environment:?})"
    )]
    ExtraCaOutsideDevelopment { environment: Environment },
}

/// A relay to connect to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEndpoint {
    host: String,
    port: u16,
    region: String,
    environment: Environment,
    extra_ca: Option<ExtraCa>,
}

impl RelayEndpoint {
    /// Endpoint `host:port`. `region` is a display label (e.g. "EU West"); `environment` is the
    /// deployment environment this client is configured for.
    pub fn new(
        host: impl Into<String>,
        port: u16,
        region: impl Into<String>,
        environment: Environment,
    ) -> Result<Self, EndpointError> {
        let host = host.into().trim().to_owned();
        validate_host(&host)?;
        if port == 0 {
            return Err(EndpointError::InvalidPort);
        }
        Ok(Self {
            host,
            port,
            region: region.into(),
            environment,
            extra_ca: None,
        })
    }

    /// Parses `host`, `host:port`, `[ipv6]` or `[ipv6]:port`; the port defaults to
    /// [`protocol::DEFAULT_PORT`] (443).
    pub fn parse(
        address: &str,
        region: impl Into<String>,
        environment: Environment,
    ) -> Result<Self, EndpointError> {
        let address = address.trim();
        if address.contains(SCHEME_MARKER) {
            return Err(EndpointError::UrlNotHost);
        }
        let (host, port) = split_host_port(address)?;
        Self::new(host, port, region, environment)
    }

    /// Adds an extra trusted CA. Refused unless the environment is development (D-016).
    pub fn with_extra_ca(mut self, ca: ExtraCa) -> Result<Self, EndpointError> {
        if self.environment != Environment::Development {
            return Err(EndpointError::ExtraCaOutsideDevelopment {
                environment: self.environment,
            });
        }
        self.extra_ca = Some(ca);
        Ok(self)
    }

    /// Host name or IP address (also the TLS server name).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// TCP port.
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Display label of the relay region.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The environment this client is configured for.
    pub const fn environment(&self) -> Environment {
        self.environment
    }

    /// The extra trusted CA (development only).
    pub const fn extra_ca(&self) -> Option<&ExtraCa> {
        self.extra_ca.as_ref()
    }

    /// Namespace under which this relay's credentials are stored: tokens of one relay or
    /// environment are never offered to another.
    pub fn credential_namespace(&self) -> String {
        format!(
            "{}/{}:{}",
            environment_name(self.environment),
            self.host.to_ascii_lowercase(),
            self.port
        )
    }
}

impl fmt::Display for RelayEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(PORT_SEPARATOR) {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// Lower-case name of an environment (stable: used in credential keys).
pub const fn environment_name(environment: Environment) -> &'static str {
    match environment {
        Environment::Development => "development",
        Environment::Staging => "staging",
        Environment::Production => "production",
    }
}

fn validate_host(host: &str) -> Result<(), EndpointError> {
    if host.is_empty() {
        return Err(EndpointError::EmptyHost);
    }
    if host.contains(SCHEME_MARKER) || host.contains('/') {
        return Err(EndpointError::UrlNotHost);
    }
    if ServerName::try_from(host).is_err() || host.chars().any(char::is_whitespace) {
        return Err(EndpointError::InvalidHost {
            host: host.to_owned(),
        });
    }
    Ok(())
}

fn split_host_port(address: &str) -> Result<(&str, u16), EndpointError> {
    if let Some(rest) = address.strip_prefix(IPV6_OPEN) {
        let Some((host, after)) = rest.split_once(IPV6_CLOSE) else {
            return Err(EndpointError::InvalidHost {
                host: address.to_owned(),
            });
        };
        return match after.strip_prefix(PORT_SEPARATOR) {
            None if after.is_empty() => Ok((host, protocol::DEFAULT_PORT)),
            None => Err(EndpointError::InvalidHost {
                host: address.to_owned(),
            }),
            Some(port) => Ok((host, parse_port(port)?)),
        };
    }
    // A bare IPv6 literal has several colons and no port.
    if address.matches(PORT_SEPARATOR).count() > 1 {
        return Ok((address, protocol::DEFAULT_PORT));
    }
    match address.split_once(PORT_SEPARATOR) {
        None => Ok((address, protocol::DEFAULT_PORT)),
        Some((host, port)) => Ok((host, parse_port(port)?)),
    }
}

fn parse_port(port: &str) -> Result<u16, EndpointError> {
    match port.parse::<u16>() {
        Ok(p) if p != 0 => Ok(p),
        _ => Err(EndpointError::InvalidPort),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_port_forms() {
        let e = RelayEndpoint::parse("relay.example.com", "EU", Environment::Production)
            .expect("valid");
        assert_eq!((e.host(), e.port()), ("relay.example.com", 443));
        let e = RelayEndpoint::parse(" relay.example.com:8443 ", "EU", Environment::Production)
            .expect("valid");
        assert_eq!((e.host(), e.port()), ("relay.example.com", 8443));
        let e = RelayEndpoint::parse("[::1]:9000", "local", Environment::Development).expect("v6");
        assert_eq!((e.host(), e.port()), ("::1", 9000));
        assert_eq!(e.to_string(), "[::1]:9000");
        let e = RelayEndpoint::parse("::1", "local", Environment::Development).expect("bare v6");
        assert_eq!((e.host(), e.port()), ("::1", 443));
        let e =
            RelayEndpoint::parse("127.0.0.1:4433", "local", Environment::Development).expect("v4");
        assert_eq!(e.to_string(), "127.0.0.1:4433");
    }

    #[test]
    fn rejects_bad_endpoints() {
        use EndpointError::*;
        let env = Environment::Production;
        assert_eq!(RelayEndpoint::parse("", "x", env), Err(EmptyHost));
        assert_eq!(
            RelayEndpoint::parse("https://relay.example.com", "x", env),
            Err(UrlNotHost)
        );
        assert_eq!(RelayEndpoint::parse("relay:0", "x", env), Err(InvalidPort));
        assert_eq!(
            RelayEndpoint::parse("relay:http", "x", env),
            Err(InvalidPort)
        );
        assert!(matches!(
            RelayEndpoint::new("bad host", 443, "x", env),
            Err(InvalidHost { .. })
        ));
        assert_eq!(RelayEndpoint::new("relay", 0, "x", env), Err(InvalidPort));
    }

    #[test]
    fn extra_ca_only_in_development() {
        let ca = ExtraCa::Pem(b"-----BEGIN CERTIFICATE-----".to_vec());
        for env in [Environment::Staging, Environment::Production] {
            let e = RelayEndpoint::new("relay.example.com", 443, "x", env).expect("valid");
            assert_eq!(
                e.with_extra_ca(ca.clone()),
                Err(EndpointError::ExtraCaOutsideDevelopment { environment: env })
            );
        }
        let dev = RelayEndpoint::new("localhost", 4433, "x", Environment::Development)
            .expect("valid")
            .with_extra_ca(ca)
            .expect("development may add a CA");
        assert!(dev.extra_ca().is_some());
        assert!(format!("{:?}", dev.extra_ca()).contains("bytes"));
    }

    #[test]
    fn credential_namespace_separates_relays_and_environments() {
        let a = RelayEndpoint::new("Relay.Example.com", 443, "x", Environment::Production)
            .expect("valid");
        let b =
            RelayEndpoint::new("relay.example.com", 443, "x", Environment::Staging).expect("valid");
        assert_eq!(a.credential_namespace(), "production/relay.example.com:443");
        assert_ne!(a.credential_namespace(), b.credential_namespace());
    }
}
