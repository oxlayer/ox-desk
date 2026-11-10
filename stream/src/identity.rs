//! Who is on the other end of the screen channel.
//!
//! Spec: ../specs/017-product.md (section 1, phase A)
//!
//! Every installation has one identity: a self-signed certificate and its
//! key (ECDSA P-256), created once and kept. The fingerprint is the SHA-256
//! of the certificate DER. The certificate is stored, and not regenerated,
//! because ECDSA signatures are randomized: regenerating from the same key
//! would give a different DER, and so a different fingerprint.
//!
//! Phase A only observes on the host side: it asks for the client
//! certificate without requiring it and logs who connected. The client side
//! already enforces when it has a pinned fingerprint (`OXDESK_PIN`).
//!
//! Signatures are verified for real in both directions. The old verifier
//! returned `assertion()` for the handshake signature too, so pinning the
//! certificate alone would have accepted anyone replaying the host's
//! (public) certificate without its key.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

const FILE: &str = "identity.der";
pub const AUTHORIZED_FILE: &str = "authorized";

pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
}

impl Identity {
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.cert)
    }
}

/// Lowercase hex SHA-256 of the certificate DER.
pub fn fingerprint(cert: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, cert)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The first 12 hex digits, for logs. Enough to tell machines apart, and the
/// full value is one `--digital` away.
pub fn short(fp: &str) -> &str {
    &fp[..fp.len().min(12)]
}

/// `OXDESK_IDENTITY_DIR`, or the per-OS config dir plus `identidade`.
pub fn dir() -> PathBuf {
    if let Ok(d) = std::env::var("OXDESK_IDENTITY_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    #[cfg(windows)]
    let base = PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()));
    #[cfg(not(windows))]
    let base = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config");
    base.join("oxdesk").join("identity")
}

/// This installation's identity, created on first use.
pub fn load() -> Result<Identity> {
    load_or_create(&dir())
}

/// File format: `[u32 BE cert length][cert DER][PKCS#8 key DER]`.
///
/// Creation is race-free: the new identity is written to a unique temporary
/// file and published with `hard_link`, which fails if the file already
/// exists. The loser of a race (video and input endpoints start together)
/// loads the winner's identity instead of keeping its own.
pub fn load_or_create(dir: &Path) -> Result<Identity> {
    let path = dir.join(FILE);
    if let Ok(bytes) = std::fs::read(&path) {
        return parse(&bytes).with_context(|| format!("corrupt identity at {}", path.display()));
    }
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;

    let key_pair = rcgen::KeyPair::generate()?;
    let params = rcgen::CertificateParams::new(vec!["oxdesk-stream".into()])?;
    let cert = params.self_signed(&key_pair)?;
    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();

    let mut bytes = Vec::with_capacity(4 + cert_der.len() + key_der.len());
    bytes.extend_from_slice(&(cert_der.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&cert_der);
    bytes.extend_from_slice(&key_der);

    let tmp = dir.join(format!(".{FILE}.{}.{}", std::process::id(), nonce()));
    write_private(&tmp, &bytes)?;
    let linked = std::fs::hard_link(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => parse(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            parse(&std::fs::read(&path)?).context("identity created by another process")
        }
        Err(e) => Err(e).with_context(|| format!("publish {}", path.display())),
    }
}

/// Unique within the process too: the clock alone repeats between threads
/// (macOS gives microseconds), and two threads racing to create the
/// identity is exactly the case this exists for.
fn nonce() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t}-{}", SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path).with_context(|| format!("write {}", path.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

fn parse(bytes: &[u8]) -> Result<Identity> {
    anyhow::ensure!(bytes.len() > 4, "file too short");
    let n = u32::from_be_bytes(bytes[..4].try_into()?) as usize;
    anyhow::ensure!(bytes.len() > 4 + n, "invalid certificate length");
    let cert = CertificateDer::from(bytes[4..4 + n].to_vec());
    let key = PrivateKeyDer::try_from(bytes[4 + n..].to_vec()).map_err(|e| anyhow::anyhow!("key: {e}"))?;
    Ok(Identity { cert, key })
}

/// Fingerprints this host accepts: one per line, `#` starts a comment.
/// A missing file is an empty list, which phase A only reports.
pub fn authorized(dir: &Path) -> HashSet<String> {
    std::fs::read_to_string(dir.join(AUTHORIZED_FILE))
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

/// What the host says about a connection's peer, for the log.
pub fn describe_peer(conn: &quinn::Connection, authorized: &HashSet<String>) -> String {
    let certs = conn
        .peer_identity()
        .and_then(|i| i.downcast::<Vec<CertificateDer<'static>>>().ok());
    match certs.as_deref().and_then(|c| c.first()) {
        None => "no certificate".into(),
        Some(c) => {
            let fp = fingerprint(c);
            let state = if authorized.contains(&fp) { "authorized" } else { "unknown" };
            format!("{} {state}", short(&fp))
        }
    }
}

fn algorithms() -> rustls::crypto::WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

/// Client side: the host must hold the key of the pinned certificate.
///
/// `pinned: None` keeps the old behaviour (any certificate) for phase A,
/// but the handshake signature is now checked either way.
#[derive(Debug)]
pub struct PinnedHost {
    pub pinned: Option<String>,
}

impl ServerCertVerifier for PinnedHost {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.pinned {
            None => Ok(ServerCertVerified::assertion()),
            Some(expected) if *expected == fingerprint(end_entity) => Ok(ServerCertVerified::assertion()),
            // Stays pt-BR: it ends up in the handshake error that
            // `oxdesk-kvm` prints right before exiting, and oxdesk shows
            // that last line in the tray ("o KVM saiu (...) antes de agarrar").
            Some(expected) => Err(rustls::Error::General(format!(
                "a digital do host e {}, e a fixada e {}",
                short(&fingerprint(end_entity)),
                short(expected)
            ))),
        }
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &algorithms())
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &algorithms())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        algorithms().supported_schemes()
    }
}

/// The file whose presence turns phase C on for this host: clients without
/// a certificate, or with one not in `authorized`, are refused.
pub const ENFORCE_FILE: &str = "enforce";

/// Host side. Phase A/B (no `enforce` file): ask for the client certificate,
/// never require it; who it was is logged after the handshake
/// (`describe_peer`), because a client that sends nothing never reaches
/// this verifier. Phase C (`enforce` present): require it, and refuse any
/// fingerprint not in the authorized list.
///
/// Both files are read on every handshake, not once: the host of the KVM
/// lives for hours, and authorizing a machine or turning enforcement on
/// must not need a restart. It is two small reads per connection.
#[derive(Debug)]
pub struct ClientPolicy {
    pub dir: PathBuf,
}

impl ClientPolicy {
    pub fn enforcing(&self) -> bool {
        self.dir.join(ENFORCE_FILE).exists()
    }
}

impl ClientCertVerifier for ClientPolicy {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn client_auth_mandatory(&self) -> bool {
        self.enforcing()
    }
    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if self.enforcing() {
            let fp = fingerprint(end_entity);
            if !authorized(&self.dir).contains(&fp) {
                // pt-BR on purpose: it travels in the TLS alert and the
                // viewer shows it to the person.
                return Err(rustls::Error::General(format!(
                    "this computer ({}) is not authorized on this host",
                    short(&fp)
                )));
            }
        }
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &algorithms())
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &algorithms())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        algorithms().supported_schemes()
    }
}

pub fn pinned_from_env() -> Option<String> {
    std::env::var("OXDESK_PIN")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oxdesk-id-{name}-{}-{}", std::process::id(), nonce()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn identity_survives_a_restart() {
        let d = tmp("restart");
        let a = load_or_create(&d).unwrap();
        let b = load_or_create(&d).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.fingerprint().len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(d.join(FILE)).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600, "the key must not be readable by others");
        }
    }

    #[test]
    fn concurrent_creation_yields_one_identity() {
        let d = tmp("race");
        let fps: Vec<String> = (0..8)
            .map(|_| {
                let d = d.clone();
                std::thread::spawn(move || load_or_create(&d).unwrap().fingerprint())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        assert!(fps.iter().all(|f| *f == fps[0]), "{fps:?}");
        let leftovers: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers.len(), 1, "temporaries were left behind: {leftovers:?}");
    }

    #[test]
    fn authorized_list_ignores_comments_and_case() {
        let d = tmp("list");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(AUTHORIZED_FILE), "# mac\nABC123  # mac\n\n  def \n").unwrap();
        let l = authorized(&d);
        assert_eq!(l, ["abc123".to_string(), "def".to_string()].into_iter().collect());
        assert!(authorized(&tmp("empty")).is_empty());
    }

    #[test]
    fn pin_accepts_only_its_certificate() {
        let a = load_or_create(&tmp("pin-a")).unwrap();
        let b = load_or_create(&tmp("pin-b")).unwrap();
        let v = PinnedHost { pinned: Some(a.fingerprint()) };
        let name = ServerName::try_from("oxdesk-stream").unwrap();
        assert!(v.verify_server_cert(&a.cert, &[], &name, &[], UnixTime::now()).is_ok());
        assert!(v.verify_server_cert(&b.cert, &[], &name, &[], UnixTime::now()).is_err());
    }
}
