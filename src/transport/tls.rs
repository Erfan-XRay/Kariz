//! TLS for the `wss` transport: rustls with the `ring` provider (the same crate as the
//! AEAD and X25519 code, and it builds cleanly for musl).
//!
//! * Dialing side: the server certificate is checked against the Mozilla roots
//!   (`webpki-roots`) by default. `tls.pin_sha256` accepts exactly one certificate
//!   instead (for self-signed ones), `tls.insecure` accepts any. The handshake
//!   signatures are verified in every case. The server name (SNI) is `tls.sni`, else the
//!   `ws.host` host, else the host of `remote`, so `remote` can be a bare CDN IP.
//! * Listening side: `tls.cert` / `tls.key` (PEM). They are loaded at startup, and again
//!   whenever either file's modification time changes, so certificate renewals need no
//!   restart. A broken pair (e.g. halfway through a renewal) keeps the previous one.
//!
//! Both sides offer ALPN `http/1.1`, as a browser opening a WebSocket does.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{info, warn};

use crate::config::TlsConfig;

const ALPN_HTTP11: &[u8] = b"http/1.1";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Hex SHA-256 of a certificate's DER encoding: the format of `tls.pin_sha256`.
pub fn cert_sha256(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// `tls.pin_sha256` for the first certificate in a PEM file (`kariz pin`).
pub fn pin_of_file(path: &Path) -> io::Result<String> {
    let cert = CertificateDer::from_pem_file(path).map_err(|e| pem_error(path, e))?;
    Ok(cert_sha256(&cert))
}

/// Dialing side: a connector plus the server name it presents.
#[derive(Clone)]
pub struct Client {
    connector: TlsConnector,
    server_name: ServerName<'static>,
}

impl Client {
    /// `default_name` is used when `tls.sni` is not set (a host name or IP address,
    /// without a port).
    pub fn new(tls: Option<&TlsConfig>, default_name: &str) -> io::Result<Self> {
        let provider = provider();
        let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?;
        let pin = tls.and_then(|t| t.pin_sha256.as_deref());
        let insecure = tls.is_some_and(|t| t.insecure);
        let mut config = if pin.is_some() || insecure {
            let verifier = Pinned {
                pin: pin.map(str::to_ascii_lowercase),
                algorithms: provider.signature_verification_algorithms,
            };
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth()
        } else {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            builder.with_root_certificates(roots).with_no_client_auth()
        };
        config.alpn_protocols = vec![ALPN_HTTP11.to_vec()];

        let name = tls.and_then(|t| t.sni.as_deref()).unwrap_or(default_name);
        let server_name = ServerName::try_from(name.to_owned()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name:?} is not a valid TLS server name"),
            )
        })?;
        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
            server_name,
        })
    }

    pub async fn connect<S>(&self, stream: S) -> io::Result<tokio_rustls::client::TlsStream<S>>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        self.connector
            .connect(self.server_name.clone(), stream)
            .await
    }
}

/// Accepts the certificate whose SHA-256 is `pin`, or any certificate when `pin` is
/// `None` (`tls.insecure`). Handshake signatures are still verified, so the server
/// must hold the certificate's private key.
#[derive(Debug)]
struct Pinned {
    pin: Option<String>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.pin {
            Some(pin) if cert_sha256(end_entity) != *pin => Err(rustls::Error::General(
                "server certificate does not match tunnel.tls.pin_sha256".into(),
            )),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Listening side: loads `cert` and `key` (failing now if they are unusable) and
/// returns an acceptor that picks up changes to them.
pub fn acceptor(cert: &Path, key: &Path) -> io::Result<TlsAcceptor> {
    let provider = provider();
    let resolver = ReloadingCert::new(cert, key, provider.clone())?;
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    config.alpn_protocols = vec![ALPN_HTTP11.to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Serves the certificate from `cert` / `key`, reloading them when their modification
/// times change. Checked on every handshake: two `stat` calls, far cheaper than the
/// handshake itself.
struct ReloadingCert {
    cert: PathBuf,
    key: PathBuf,
    provider: Arc<CryptoProvider>,
    current: Mutex<Loaded>,
}

struct Loaded {
    key: Arc<CertifiedKey>,
    /// Modification times the last load attempt saw.
    stamp: Stamp,
}

type Stamp = (Option<SystemTime>, Option<SystemTime>);

impl ReloadingCert {
    fn new(cert: &Path, key: &Path, provider: Arc<CryptoProvider>) -> io::Result<Self> {
        let stamp = stamp(cert, key);
        let loaded = load(cert, key, &provider)?;
        Ok(Self {
            cert: cert.to_owned(),
            key: key.to_owned(),
            provider,
            current: Mutex::new(Loaded { key: loaded, stamp }),
        })
    }
}

fn stamp(cert: &Path, key: &Path) -> Stamp {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    (mtime(cert), mtime(key))
}

fn load(cert: &Path, key: &Path, provider: &CryptoProvider) -> io::Result<Arc<CertifiedKey>> {
    let chain = CertificateDer::pem_file_iter(cert)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .map_err(|e| pem_error(cert, e))?;
    if chain.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no certificate found in {}", cert.display()),
        ));
    }
    let private = PrivateKeyDer::from_pem_file(key).map_err(|e| pem_error(key, e))?;
    let certified = CertifiedKey::from_der(chain, private, provider).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} and {} do not form a usable certificate: {e}",
                cert.display(),
                key.display()
            ),
        )
    })?;
    Ok(Arc::new(certified))
}

impl ResolvesServerCert for ReloadingCert {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let stamp = stamp(&self.cert, &self.key);
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if stamp != current.stamp {
            // Retried on the next change of either file, not on every handshake.
            current.stamp = stamp;
            match load(&self.cert, &self.key, &self.provider) {
                Ok(key) => {
                    info!(cert = %self.cert.display(), "TLS certificate reloaded");
                    current.key = key;
                }
                Err(e) => warn!(error = %e, "TLS certificate not reloaded, keeping the old one"),
            }
        }
        Some(current.key.clone())
    }
}

impl fmt::Debug for ReloadingCert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReloadingCert")
            .field("cert", &self.cert)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

fn pem_error(path: &Path, e: rustls::pki_types::pem::Error) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("cannot read {}: {e}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    struct TestCert {
        cert_pem: String,
        key_pem: String,
        pin: String,
    }

    fn test_cert(name: &str) -> TestCert {
        let c = rcgen::generate_simple_self_signed(vec![name.to_owned()]).unwrap();
        TestCert {
            pin: cert_sha256(c.cert.der()),
            cert_pem: c.cert.pem(),
            key_pem: c.signing_key.serialize_pem(),
        }
    }

    /// A fresh directory for one test's files.
    fn temp_dir() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kariz-tls-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes the pair and gives both files the modification time `at` (seconds after
    /// an arbitrary base), so changes are seen whatever the file system's resolution.
    fn install(dir: &Path, c: &TestCert, at: u64) -> (PathBuf, PathBuf) {
        let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + at);
        for (path, text) in [(&cert, &c.cert_pem), (&key, &c.key_pem)] {
            std::fs::write(path, text).unwrap();
            let file = std::fs::File::options().write(true).open(path).unwrap();
            file.set_modified(time).unwrap();
        }
        (cert, key)
    }

    fn client(extra: &str) -> io::Result<Client> {
        let tls: TlsConfig = toml::from_str(extra).unwrap();
        Client::new(Some(&tls), "tunnel.example")
    }

    /// One TLS connection over an in-memory pipe; returns what the server saw (SNI,
    /// ALPN) once a byte went through in each direction.
    async fn handshake(
        acceptor: &TlsAcceptor,
        client: &Client,
    ) -> io::Result<(Option<String>, Option<Vec<u8>>)> {
        let (c, s) = duplex(1 << 16);
        let acceptor = acceptor.clone();
        let server = tokio::spawn(async move {
            let mut tls = acceptor.accept(s).await?;
            let mut b = [0u8; 1];
            tls.read_exact(&mut b).await?;
            tls.write_all(&b).await?;
            tls.flush().await?;
            let conn = tls.get_ref().1;
            io::Result::Ok((
                conn.server_name().map(str::to_owned),
                conn.alpn_protocol().map(<[u8]>::to_vec),
            ))
        });
        let result = async {
            let mut tls = client.connect(c).await?;
            tls.write_all(b"x").await?;
            tls.flush().await?;
            let mut b = [0u8; 1];
            tls.read_exact(&mut b).await?;
            io::Result::Ok(())
        }
        .await;
        let seen = server.await.unwrap();
        result?;
        seen
    }

    #[tokio::test]
    async fn verification_modes() {
        let a = test_cert("tunnel.example");
        let b = test_cert("tunnel.example");
        let (cert, key) = install(&temp_dir(), &a, 0);
        let acceptor = acceptor(&cert, &key).unwrap();

        let pinned = client(&format!("pin_sha256 = \"{}\"", a.pin)).unwrap();
        let (sni, alpn) = handshake(&acceptor, &pinned).await.unwrap();
        assert_eq!(sni.as_deref(), Some("tunnel.example"));
        assert_eq!(alpn.as_deref(), Some(ALPN_HTTP11));
        // Upper-case hex is the same pin.
        let upper = client(&format!("pin_sha256 = \"{}\"", a.pin.to_uppercase())).unwrap();
        assert!(handshake(&acceptor, &upper).await.is_ok());

        let wrong = client(&format!("pin_sha256 = \"{}\"", b.pin)).unwrap();
        let err = handshake(&acceptor, &wrong).await.unwrap_err();
        assert!(err.to_string().contains("pin_sha256"), "{err}");

        let insecure = client("insecure = true").unwrap();
        assert!(handshake(&acceptor, &insecure).await.is_ok());

        // Default: a self-signed certificate is not trusted.
        let default = Client::new(None, "tunnel.example").unwrap();
        let err = handshake(&acceptor, &default).await.unwrap_err();
        assert!(err.to_string().contains("certificate"), "{err}");
    }

    #[tokio::test]
    async fn sni_comes_from_config_or_default_name() {
        let a = test_cert("front.example");
        let (cert, key) = install(&temp_dir(), &a, 0);
        let acceptor = acceptor(&cert, &key).unwrap();
        let pin = format!("pin_sha256 = \"{}\"", a.pin);

        let c = client(&format!("{pin}\nsni = \"front.example\"")).unwrap();
        let (sni, _) = handshake(&acceptor, &c).await.unwrap();
        assert_eq!(sni.as_deref(), Some("front.example"));

        // An IP address as the name: no SNI is sent, as browsers do.
        let tls: TlsConfig = toml::from_str(&pin).unwrap();
        let c = Client::new(Some(&tls), "127.0.0.1").unwrap();
        let (sni, _) = handshake(&acceptor, &c).await.unwrap();
        assert_eq!(sni, None);

        assert!(Client::new(Some(&tls), "bad name!").is_err());
    }

    #[tokio::test]
    async fn certificate_is_reloaded_when_the_files_change() {
        let dir = temp_dir();
        let (a, b) = (test_cert("tunnel.example"), test_cert("tunnel.example"));
        let (cert, key) = install(&dir, &a, 0);
        let acceptor = acceptor(&cert, &key).unwrap();
        let pinned = |c: &TestCert| client(&format!("pin_sha256 = \"{}\"", c.pin)).unwrap();
        assert!(handshake(&acceptor, &pinned(&a)).await.is_ok());

        install(&dir, &b, 10);
        assert!(handshake(&acceptor, &pinned(&b)).await.is_ok());
        assert!(handshake(&acceptor, &pinned(&a)).await.is_err());

        // A half-written renewal (new certificate, old key) keeps serving b ...
        let mixed = TestCert {
            cert_pem: a.cert_pem.clone(),
            key_pem: b.key_pem.clone(),
            pin: String::new(),
        };
        install(&dir, &mixed, 20);
        assert!(handshake(&acceptor, &pinned(&b)).await.is_ok());
        // ... until the pair is complete again.
        install(&dir, &a, 30);
        assert!(handshake(&acceptor, &pinned(&a)).await.is_ok());
    }

    #[test]
    fn unusable_files_fail_at_startup() {
        let dir = temp_dir();
        let (a, b) = (test_cert("x.example"), test_cert("x.example"));
        let (cert, key) = install(&dir, &a, 0);
        assert!(acceptor(&cert, &dir.join("missing.pem")).is_err());
        assert!(acceptor(&key, &key).is_err(), "a key is not a certificate");
        std::fs::write(&key, &b.key_pem).unwrap();
        let err = acceptor(&cert, &key).err().unwrap();
        assert!(err.to_string().contains("usable certificate"), "{err}");
    }

    #[test]
    fn pin_of_a_pem_file() {
        let a = test_cert("x.example");
        let (cert, _) = install(&temp_dir(), &a, 0);
        assert_eq!(pin_of_file(&cert).unwrap(), a.pin);
        assert_eq!(a.pin.len(), 64);
    }
}
