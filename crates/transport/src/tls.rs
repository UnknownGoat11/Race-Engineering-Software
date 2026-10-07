//! TLS client configuration (D-004, D-016, ARCHITECTURE.md §7).
//!
//! * TLS 1.3 only, rustls with the `ring` provider, ALPN [`protocol::ALPN_PROTOCOL`] (`lmure/1`).
//! * Trust store: the Mozilla roots of `webpki-roots`, plus — **only** for
//!   [`Environment::Development`] — the extra CA of the endpoint (a self-hosted development relay).
//! * There is no code path that disables or weakens certificate validation: this module never
//!   installs a custom `ServerCertVerifier`.

use std::sync::Arc;

use protocol::version::Environment;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};

use crate::endpoint::{ExtraCa, RelayEndpoint};
use crate::error::{ConnectError, TlsConfigError};

/// Builds the TLS client configuration for `endpoint`.
pub fn client_config(endpoint: &RelayEndpoint) -> Result<Arc<ClientConfig>, TlsConfigError> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(extra) = endpoint.extra_ca() {
        // RelayEndpoint::with_extra_ca already refuses this; checked again here so that no other
        // construction path can ever trust a private CA outside development.
        if endpoint.environment() != Environment::Development {
            return Err(TlsConfigError::ExtraCaOutsideDevelopment {
                environment: endpoint.environment(),
            });
        }
        for cert in extra_ca_certificates(extra)? {
            roots
                .add(cert)
                .map_err(|source| TlsConfigError::ExtraCaRejected { source })?;
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|source| TlsConfigError::Tls13Unavailable { source })?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![protocol::ALPN_PROTOCOL.to_vec()];
    Ok(Arc::new(config))
}

/// The TLS server name (SNI and certificate name check) of `endpoint`.
pub fn server_name(endpoint: &RelayEndpoint) -> Result<ServerName<'static>, TlsConfigError> {
    ServerName::try_from(endpoint.host().to_owned()).map_err(|_| TlsConfigError::ServerName {
        host: endpoint.host().to_owned(),
    })
}

/// Classifies a failed client handshake: certificate problems are reported separately from other
/// TLS failures so the UI can say "the relay's certificate is not trusted".
pub fn classify_handshake_error(error: std::io::Error) -> ConnectError {
    let certificate_error = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        .filter(|e| {
            matches!(
                e,
                rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented
            )
        })
        .cloned();
    match certificate_error {
        Some(source) => ConnectError::UntrustedCertificate { source },
        None => ConnectError::Tls { source: error },
    }
}

/// Whether the negotiated ALPN protocol is `lmure/1`.
pub fn alpn_ok(negotiated: Option<&[u8]>) -> bool {
    negotiated == Some(protocol::ALPN_PROTOCOL)
}

fn extra_ca_certificates(ca: &ExtraCa) -> Result<Vec<CertificateDer<'static>>, TlsConfigError> {
    let pem = match ca {
        ExtraCa::PemFile(path) => {
            std::fs::read(path).map_err(|source| TlsConfigError::ExtraCaRead {
                path: path.clone(),
                source,
            })?
        }
        ExtraCa::Pem(bytes) => bytes.clone(),
    };
    let certs = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| TlsConfigError::ExtraCaPem { source })?;
    if certs.is_empty() {
        return Err(TlsConfigError::ExtraCaEmpty);
    }
    Ok(certs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(env: Environment) -> RelayEndpoint {
        RelayEndpoint::new("localhost", 4433, "test", env).expect("valid endpoint")
    }

    #[test]
    fn production_config_is_tls13_only_with_alpn_and_public_roots() {
        let config = client_config(&endpoint(Environment::Production)).expect("config");
        assert_eq!(config.alpn_protocols, vec![b"lmure/1".to_vec()]);
        assert!(
            config
                .crypto_provider()
                .cipher_suites
                .iter()
                .any(|s| s.version() == &rustls::version::TLS13)
        );
        // TLS 1.3 only: the handshake tests (tests/tls.rs) also check the negotiated version.
    }

    #[test]
    fn extra_ca_errors_are_reported() {
        let dev = endpoint(Environment::Development)
            .with_extra_ca(ExtraCa::Pem(b"not a pem".to_vec()))
            .expect("dev");
        assert!(matches!(
            client_config(&dev),
            Err(TlsConfigError::ExtraCaEmpty)
        ));
        let missing = endpoint(Environment::Development)
            .with_extra_ca(ExtraCa::PemFile("does/not/exist.pem".into()))
            .expect("dev");
        assert!(matches!(
            client_config(&missing),
            Err(TlsConfigError::ExtraCaRead { .. })
        ));
    }

    #[test]
    fn alpn_must_match_exactly() {
        assert!(alpn_ok(Some(b"lmure/1")));
        assert!(!alpn_ok(None));
        assert!(!alpn_ok(Some(b"h2")));
    }

    #[test]
    fn handshake_errors_are_classified() {
        let cert = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
        );
        assert!(matches!(
            classify_handshake_error(cert),
            ConnectError::UntrustedCertificate { .. }
        ));
        let other = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        assert!(matches!(
            classify_handshake_error(other),
            ConnectError::Tls { .. }
        ));
    }
}
