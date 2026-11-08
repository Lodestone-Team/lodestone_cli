//! TLS for the API client: system trust, a pinned certificate (trust on first use), or a
//! CA certificate from `--ca-cert`. Certificate checks are never turned off.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_pki_types::pem::PemObject;

use super::config::TlsSettings;
use super::error::{kind, CliError, CliResult};

/// How long connecting and the TLS handshake may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Which certificates the client accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// Certificates the operating system trusts, for the host name in the URL.
    System,
    /// Only the certificate with this SHA-256 fingerprint.
    Pinned([u8; 32]),
    /// Certificates signed by these CAs (DER), for the host name in the URL.
    CaCerts(Vec<Vec<u8>>),
}

impl Trust {
    pub fn from_settings(settings: &TlsSettings) -> CliResult<Self> {
        if let Some(pin) = &settings.pinned_sha256 {
            return Ok(Trust::Pinned(parse_fingerprint(pin)?));
        }
        if let Some(pem) = &settings.ca_cert_pem {
            return Ok(Trust::CaCerts(parse_pem_certs(pem.as_bytes())?));
        }
        Ok(Trust::System)
    }
}

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// SHA-256 of a DER certificate.
pub fn sha256(der: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// A fingerprint as the core and browsers show it: colon-separated uppercase hex.
pub fn format_fingerprint(fingerprint: &[u8]) -> String {
    fingerprint
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Parses a SHA-256 fingerprint typed by a user: hex in any case, with or without colons
/// or spaces, optionally prefixed with `sha256:`.
pub fn parse_fingerprint(input: &str) -> CliResult<[u8; 32]> {
    let trimmed = input.trim();
    let trimmed = trimmed
        .strip_prefix("sha256:")
        .or_else(|| trimmed.strip_prefix("SHA256:"))
        .unwrap_or(trimmed);
    let hex: String = trimmed
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .collect();
    let invalid = || {
        CliError::usage(format!(
            "`{input}` is not a SHA-256 fingerprint (64 hex digits, such as AB:CD:...)"
        ))
    };
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| invalid())?;
    }
    Ok(out)
}

pub fn parse_pem_certs(pem: &[u8]) -> CliResult<Vec<Vec<u8>>> {
    let mut certs = Vec::new();
    for cert in CertificateDer::pem_slice_iter(pem) {
        let cert = cert.map_err(|e| {
            CliError::new(kind::TLS, format!("Invalid PEM certificate file: {e:?}"))
        })?;
        certs.push(cert.as_ref().to_vec());
    }
    if certs.is_empty() {
        return Err(CliError::new(
            kind::TLS,
            "The CA certificate file contains no PEM certificate",
        ));
    }
    Ok(certs)
}

fn system_roots() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    roots.add_parsable_certificates(native.certs);
    roots
}

fn webpki_verifier(roots: RootCertStore) -> CliResult<Arc<WebPkiServerVerifier>> {
    WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
        .build()
        .map_err(|e| CliError::new(kind::TLS, format!("No usable root certificates: {e}")))
}

fn verifier_for(trust: &Trust) -> CliResult<Arc<dyn ServerCertVerifier>> {
    Ok(match trust {
        Trust::System => webpki_verifier(system_roots())?,
        Trust::CaCerts(certs) => {
            let mut roots = RootCertStore::empty();
            for cert in certs {
                roots.add(CertificateDer::from(cert.clone())).map_err(|e| {
                    CliError::new(kind::TLS, format!("Invalid CA certificate: {e}"))
                })?;
            }
            webpki_verifier(roots)?
        }
        Trust::Pinned(fingerprint) => Arc::new(PinnedVerifier {
            fingerprint: *fingerprint,
            provider: provider(),
        }),
    })
}

fn config_with_verifier(verifier: Arc<dyn ServerCertVerifier>) -> CliResult<ClientConfig> {
    Ok(ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| CliError::new(kind::TLS, format!("TLS setup failed: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

/// The rustls client config for `trust`.
pub fn client_config(trust: &Trust) -> CliResult<ClientConfig> {
    config_with_verifier(verifier_for(trust)?)
}

/// Accepts exactly one certificate, by its SHA-256 fingerprint, whatever its issuer, names
/// or dates. Handshake signatures are still checked, so the server must hold the key.
#[derive(Debug)]
struct PinnedVerifier {
    fingerprint: [u8; 32],
    provider: Arc<CryptoProvider>,
}

#[derive(Debug)]
struct FingerprintMismatch {
    expected: String,
    actual: String,
}

impl std::fmt::Display for FingerprintMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the core's certificate changed: its SHA-256 fingerprint is {}, but this profile trusts only {}",
            self.actual, self.expected
        )
    }
}

impl std::error::Error for FingerprintMismatch {}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = sha256(end_entity.as_ref());
        // fingerprints are public, so a plain comparison is fine
        if actual == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                    FingerprintMismatch {
                        expected: format_fingerprint(&self.fingerprint),
                        actual: format_fingerprint(&actual),
                    },
                ))),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Wraps a verifier and remembers the certificate the server sent, so a failed handshake
/// can still show its fingerprint.
#[derive(Debug)]
struct RecordingVerifier {
    inner: Arc<dyn ServerCertVerifier>,
    seen: Mutex<Option<Vec<u8>>>,
}

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(end_entity.as_ref().to_vec());
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// The TLS server name for a URL's host.
pub fn server_name(url: &reqwest::Url) -> CliResult<ServerName<'static>> {
    match url.host() {
        Some(url::Host::Domain(domain)) => ServerName::try_from(domain.to_string())
            .map_err(|_| CliError::usage(format!("`{domain}` is not a valid host name"))),
        Some(url::Host::Ipv4(ip)) => Ok(ServerName::IpAddress(std::net::IpAddr::V4(ip).into())),
        Some(url::Host::Ipv6(ip)) => Ok(ServerName::IpAddress(std::net::IpAddr::V6(ip).into())),
        None => Err(CliError::usage(format!("`{url}` has no host"))),
    }
}

/// Opens a TCP connection to the URL's host and port.
pub async fn connect_tcp(url: &reqwest::Url) -> CliResult<tokio::net::TcpStream> {
    let host = url
        .host_str()
        .ok_or_else(|| CliError::usage(format!("`{url}` has no host")))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| CliError::usage(format!("`{url}` has no port")))?;
    let connect = tokio::net::TcpStream::connect((host.as_str(), port));
    match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(CliError::new(
            kind::NETWORK,
            format!("Could not connect to {host}:{port}: {e}"),
        )),
        Err(_) => Err(CliError::new(
            kind::NETWORK,
            format!("Timed out connecting to {host}:{port}"),
        )),
    }
}

/// Runs a TLS handshake over `stream`.
pub async fn handshake(
    config: Arc<ClientConfig>,
    url: &reqwest::Url,
    stream: tokio::net::TcpStream,
) -> CliResult<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let connector = tokio_rustls::TlsConnector::from(config);
    let name = server_name(url)?;
    match tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, stream)).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(tls_error(&e)),
        Err(_) => Err(CliError::new(
            kind::NETWORK,
            "Timed out during the TLS handshake",
        )),
    }
}

/// A readable error for a failed handshake.
pub fn tls_error(e: &std::io::Error) -> CliError {
    let rustls_error = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>());
    match rustls_error {
        Some(error) => CliError::new(
            if is_certificate_error(error) {
                kind::UNTRUSTED_CERTIFICATE
            } else {
                kind::TLS
            },
            format!("TLS handshake failed: {}", describe_rustls_error(error)),
        ),
        None => CliError::new(kind::TLS, format!("TLS handshake failed: {e}")),
    }
}

/// Whether a TLS error is about the server's certificate (as opposed to the protocol).
pub fn is_certificate_error(error: &rustls::Error) -> bool {
    matches!(
        error,
        rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented
    )
}

/// A rustls error for people: our own verifier errors (such as a changed pinned
/// certificate) are shown with their message rather than rustls' debug output.
pub fn describe_rustls_error(error: &rustls::Error) -> String {
    match error {
        rustls::Error::InvalidCertificate(rustls::CertificateError::Other(other)) => {
            other.0.to_string()
        }
        rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer) => {
            "the certificate is not signed by a trusted authority (it may be self-signed)"
                .to_string()
        }
        other => other.to_string(),
    }
}

/// The rustls error in an error's chain of causes, if any. `io::Error::source` skips the
/// error an `io::Error` wraps, so wrapped errors are searched too.
pub fn find_rustls_error<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> Option<&'a rustls::Error> {
    if let Some(rustls_error) = error.downcast_ref::<rustls::Error>() {
        return Some(rustls_error);
    }
    if let Some(inner) = error
        .downcast_ref::<std::io::Error>()
        .and_then(|io| io.get_ref())
    {
        let inner: &(dyn std::error::Error + 'static) = inner;
        if let Some(found) = find_rustls_error(inner) {
            return Some(found);
        }
    }
    error.source().and_then(find_rustls_error)
}

/// The result of checking a core's certificate before logging in.
#[derive(Debug, Clone)]
pub struct Probe {
    /// The fingerprint of the certificate the server sent, if it sent one.
    pub fingerprint: Option<[u8; 32]>,
    /// `None` if the certificate passed the checks of `trust`, else why not.
    pub error: Option<CliError>,
}

/// Connects to `url` and checks its certificate against `trust`, recording the
/// certificate either way. Network errors (as opposed to certificate errors) are returned
/// as `Err`.
pub async fn probe(url: &reqwest::Url, trust: &Trust) -> CliResult<Probe> {
    let recorder = Arc::new(RecordingVerifier {
        inner: verifier_for(trust)?,
        seen: Mutex::new(None),
    });
    let config = Arc::new(config_with_verifier(recorder.clone())?);
    let stream = connect_tcp(url).await?;
    let result = handshake(config, url, stream).await;
    let fingerprint = recorder
        .seen
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        .map(sha256);
    match result {
        Ok(_) => Ok(Probe {
            fingerprint,
            error: None,
        }),
        Err(e) if fingerprint.is_some() => Ok(Probe {
            fingerprint,
            error: Some(e),
        }),
        // no certificate at all: not a TLS server, or the connection broke
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_format_matches_the_core() {
        // the core's own test: sha256_fingerprint(b"abc")
        assert_eq!(
            format_fingerprint(&sha256(b"abc")),
            "BA:78:16:BF:8F:01:CF:EA:41:41:40:DE:5D:AE:22:23:B0:03:61:A3:96:17:7A:9C:B4:10:FF:61:F2:00:15:AD"
        );
    }

    #[test]
    fn fingerprint_parsing_accepts_common_forms() {
        let expected = sha256(b"abc");
        for input in [
            "BA:78:16:BF:8F:01:CF:EA:41:41:40:DE:5D:AE:22:23:B0:03:61:A3:96:17:7A:9C:B4:10:FF:61:F2:00:15:AD",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "sha256:BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
            " ba 78 16 bf 8f 01 cf ea 41 41 40 de 5d ae 22 23 b0 03 61 a3 96 17 7a 9c b4 10 ff 61 f2 00 15 ad ",
        ] {
            assert_eq!(parse_fingerprint(input).unwrap(), expected, "{input}");
        }
        for bad in ["", "AB:CD", &"Z".repeat(64), &"A".repeat(66)] {
            let e = parse_fingerprint(bad).unwrap_err();
            assert_eq!(e.kind, kind::USAGE);
        }
    }

    #[test]
    fn pinned_verifier_accepts_only_its_certificate() {
        let cert = CertificateDer::from(b"certificate bytes".to_vec());
        let other = CertificateDer::from(b"another certificate".to_vec());
        let verifier = PinnedVerifier {
            fingerprint: sha256(cert.as_ref()),
            provider: provider(),
        };
        let name = ServerName::try_from("core.example").unwrap();
        assert!(verifier
            .verify_server_cert(&cert, &[], &name, &[], UnixTime::now())
            .is_ok());
        let error = verifier
            .verify_server_cert(&other, &[], &name, &[], UnixTime::now())
            .unwrap_err();
        let message = describe_rustls_error(&error);
        assert!(message.contains("certificate changed"), "{message}");
        assert!(
            message.contains(&format_fingerprint(&sha256(other.as_ref()))),
            "{message}"
        );
    }

    #[test]
    fn trust_from_settings() {
        assert_eq!(
            Trust::from_settings(&TlsSettings::default()).unwrap(),
            Trust::System
        );
        let pin = format_fingerprint(&sha256(b"x"));
        assert_eq!(
            Trust::from_settings(&TlsSettings {
                pinned_sha256: Some(pin),
                ca_cert_pem: None
            })
            .unwrap(),
            Trust::Pinned(sha256(b"x"))
        );
        assert!(Trust::from_settings(&TlsSettings {
            pinned_sha256: None,
            ca_cert_pem: Some("not a pem".into())
        })
        .is_err());
    }
}
