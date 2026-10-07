//! TLS for the telemetry listener (D-004, D-016).
//!
//! * TLS 1.3 only (rustls, `ring` provider), ALPN `lmure/1` required.
//! * Certificate chain and private key are PEM files. [`ReloadingTls`] polls their modification
//!   time and size and swaps the configuration when they change (certbot / acme.sh renewals apply
//!   without a restart). A broken new pair is logged and the previous configuration stays active.
//! * [`write_dev_certificates`] (`lmu-relay dev-cert`) creates a development CA and a server
//!   certificate signed by it; development clients trust the CA explicitly.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Log component of this module.
const COMPONENT: &str = "relay.tls";
/// File name of the development CA certificate.
pub const DEV_CA_FILE: &str = "ca.pem";
/// File name of the development server certificate chain.
pub const DEV_CERT_FILE: &str = "server.pem";
/// File name of the development server private key.
pub const DEV_KEY_FILE: &str = "server.key.pem";
/// Names a development certificate covers when none are given.
pub const DEV_DEFAULT_NAMES: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// TLS setup failures.
#[derive(Debug, Error)]
pub enum TlsError {
    #[error("cannot read {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} contains no PEM certificate")]
    NoCertificate { path: PathBuf },
    #[error("cannot parse PEM in {path}")]
    Pem {
        path: PathBuf,
        #[source]
        source: rustls::pki_types::pem::Error,
    },
    #[error("the certificate and key are not usable together")]
    Rustls(#[source] rustls::Error),
    #[error("certificate generation failed")]
    Generate(#[source] rcgen::Error),
    #[error("cannot write {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Builds the server configuration: TLS 1.3 only, ALPN `lmure/1`, no client certificates.
pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(TlsError::Rustls)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(TlsError::Rustls)?;
    config.alpn_protocols = vec![protocol::ALPN_PROTOCOL.to_vec()];
    Ok(Arc::new(config))
}

/// Loads a PEM certificate chain and private key and builds the server configuration.
pub fn load_server_config(cert: &Path, key: &Path) -> Result<Arc<ServerConfig>, TlsError> {
    let cert_pem = fs::read(cert).map_err(|source| TlsError::Read {
        path: cert.to_owned(),
        source,
    })?;
    let chain = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| TlsError::Pem {
            path: cert.to_owned(),
            source,
        })?;
    if chain.is_empty() {
        return Err(TlsError::NoCertificate {
            path: cert.to_owned(),
        });
    }
    let key_pem = fs::read(key).map_err(|source| TlsError::Read {
        path: key.to_owned(),
        source,
    })?;
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|source| TlsError::Pem {
        path: key.to_owned(),
        source,
    })?;
    server_config(chain, key)
}

/// Change detector of one file: modification time and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp(path: &Path) -> Option<FileStamp> {
    // `metadata` follows symlinks, so a certbot `live/` link switching to a new archive file is
    // seen as a change.
    fs::metadata(path).ok().map(|m| FileStamp {
        modified: m.modified().ok(),
        len: m.len(),
    })
}

/// A server configuration loaded from PEM files and reloaded when they change.
#[derive(Debug)]
pub struct ReloadingTls {
    cert: PathBuf,
    key: PathBuf,
    current: ArcSwap<ServerConfig>,
    stamps: Mutex<(Option<FileStamp>, Option<FileStamp>)>,
}

impl ReloadingTls {
    /// Loads the pair; fails if it is unusable (startup must not continue without TLS).
    pub fn load(cert: &Path, key: &Path) -> Result<Self, TlsError> {
        let stamps = (stamp(cert), stamp(key));
        let config = load_server_config(cert, key)?;
        Ok(Self {
            cert: cert.to_owned(),
            key: key.to_owned(),
            current: ArcSwap::new(config),
            stamps: Mutex::new(stamps),
        })
    }

    /// The configuration for the next handshake.
    pub fn current(&self) -> Arc<ServerConfig> {
        self.current.load_full()
    }

    /// Reloads if either file changed since the last load. Returns `Ok(true)` if a new
    /// configuration is active, `Ok(false)` if nothing changed; on error the old one stays.
    pub fn reload_if_changed(&self) -> Result<bool, TlsError> {
        let now = (stamp(&self.cert), stamp(&self.key));
        {
            let mut seen = self.stamps.lock();
            if *seen == now {
                return Ok(false);
            }
            // Remember the attempt so a broken pair is not re-parsed every poll; the next real
            // change is picked up again.
            *seen = now;
        }
        let config = load_server_config(&self.cert, &self.key)?;
        self.current.store(config);
        Ok(true)
    }

    /// Polls the files every `interval` until `shutdown` is cancelled.
    pub async fn watch(self: Arc<Self>, interval: Duration, shutdown: CancellationToken) {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = ticker.tick() => {}
            }
            let this = Arc::clone(&self);
            // File I/O and PEM parsing off the async workers.
            match tokio::task::spawn_blocking(move || this.reload_if_changed()).await {
                Ok(Ok(true)) => tracing::info!(
                    component = COMPONENT,
                    cert = %self.cert.display(),
                    "TLS certificate reloaded"
                ),
                Ok(Ok(false)) => {}
                Ok(Err(e)) => tracing::error!(
                    component = COMPONENT,
                    cert = %self.cert.display(),
                    error = %e,
                    "TLS certificate changed but cannot be loaded; keeping the previous one"
                ),
                Err(e) => {
                    tracing::error!(component = COMPONENT, error = %e, "TLS reload task failed")
                }
            }
        }
    }
}

/// PEM text of a development CA and of a server certificate signed by it.
#[derive(Debug, Clone)]
pub struct DevCertificates {
    /// CA certificate (clients trust it in development only).
    pub ca_pem: String,
    /// Server certificate followed by the CA certificate.
    pub chain_pem: String,
    /// Server private key (PKCS#8).
    pub key_pem: String,
}

/// Generates a development CA and a server certificate for `names` (DNS names or IP literals).
pub fn generate_dev_certificates(names: &[String]) -> Result<DevCertificates, TlsError> {
    let ca_key = KeyPair::generate().map_err(TlsError::Generate)?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).map_err(TlsError::Generate)?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "lmu-relay development CA");
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_cert = ca_params.self_signed(&ca_key).map_err(TlsError::Generate)?;
    let issuer = Issuer::new(ca_params, ca_key);

    let server_key = KeyPair::generate().map_err(TlsError::Generate)?;
    let mut params = CertificateParams::new(names.to_vec()).map_err(TlsError::Generate)?;
    let common_name = names.first().map_or("localhost", String::as_str);
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let server_cert = params
        .signed_by(&server_key, &issuer)
        .map_err(TlsError::Generate)?;
    let ca_pem = ca_cert.pem();
    Ok(DevCertificates {
        chain_pem: format!("{}{}", server_cert.pem(), ca_pem),
        ca_pem,
        key_pem: server_key.serialize_pem(),
    })
}

/// Writes [`generate_dev_certificates`] output into `dir` ([`DEV_CA_FILE`], [`DEV_CERT_FILE`],
/// [`DEV_KEY_FILE`]). The CA private key is not kept: a new CA is created on every call.
pub fn write_dev_certificates(dir: &Path, names: &[String]) -> Result<DevCertificates, TlsError> {
    fs::create_dir_all(dir).map_err(|source| TlsError::Write {
        path: dir.to_owned(),
        source,
    })?;
    let certs = generate_dev_certificates(names)?;
    for (file, text) in [
        (DEV_CA_FILE, &certs.ca_pem),
        (DEV_CERT_FILE, &certs.chain_pem),
        (DEV_KEY_FILE, &certs.key_pem),
    ] {
        let path = dir.join(file);
        write_private(&path, text.as_bytes()).map_err(|source| TlsError::Write { path, source })?;
    }
    Ok(certs)
}

/// Writes a file readable only by its owner (Unix; default ACLs elsewhere).
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let suffix = crate::secrets::random_u64().expect("rng");
        let dir = std::env::temp_dir().join(format!("lmu-relay-tls-{tag}-{suffix:016x}"));
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn dev_certificates_load_as_tls13_with_alpn() {
        let dir = temp_dir("dev");
        let names: Vec<String> = DEV_DEFAULT_NAMES.iter().map(|s| (*s).to_owned()).collect();
        write_dev_certificates(&dir, &names).expect("written");
        let config =
            load_server_config(&dir.join(DEV_CERT_FILE), &dir.join(DEV_KEY_FILE)).expect("loads");
        assert_eq!(config.alpn_protocols, vec![b"lmure/1".to_vec()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_picks_up_changes_and_keeps_the_old_config_on_errors() {
        let dir = temp_dir("reload");
        let names = vec!["localhost".to_owned()];
        write_dev_certificates(&dir, &names).expect("written");
        let cert = dir.join(DEV_CERT_FILE);
        let key = dir.join(DEV_KEY_FILE);
        let tls = ReloadingTls::load(&cert, &key).expect("load");
        assert!(!tls.reload_if_changed().expect("unchanged"));
        let before = tls.current();

        // A broken certificate file: the reload fails and the previous config stays active.
        fs::write(&cert, b"garbage that is not PEM").expect("write");
        assert!(tls.reload_if_changed().is_err());
        assert!(Arc::ptr_eq(&before, &tls.current()));

        // A valid renewal is picked up.
        write_dev_certificates(&dir, &names).expect("renewed");
        assert!(tls.reload_if_changed().expect("reloaded"));
        assert!(!Arc::ptr_eq(&before, &tls.current()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_are_reported() {
        let dir = temp_dir("missing");
        assert!(matches!(
            load_server_config(&dir.join("nope.pem"), &dir.join("nope.key")),
            Err(TlsError::Read { .. })
        ));
        let empty = dir.join("empty.pem");
        fs::write(&empty, b"").expect("write");
        assert!(matches!(
            load_server_config(&empty, &empty),
            Err(TlsError::NoCertificate { .. })
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
