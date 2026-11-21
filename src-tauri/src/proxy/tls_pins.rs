//! Explicit trust for a private-CA HTTPS origin. Pins are local configuration,
//! never compiled into the distributable. All other origins retain normal TLS.

use rustls::{client::danger::ServerCertVerifier, pki_types::*};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Debug)]
struct PinnedVerifier {
    host: String,
    fingerprint: [u8; 32],
    algorithms: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let fail = || {
            rustls::Error::General("TLS certificate pin, hostname or validity check failed".into())
        };
        if server_name.to_str() != self.host
            || Sha256::digest(cert.as_ref()).as_slice() != self.fingerprint
        {
            return Err(fail());
        }
        let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref()).map_err(|_| fail())?;
        let now = x509_parser::time::ASN1Time::from_timestamp(now.as_secs() as i64)
            .map_err(|_| fail())?;
        if !parsed.validity().is_valid_at(now) {
            return Err(fail());
        }
        webpki::EndEntityCert::try_from(cert)
            .map_err(|_| fail())?
            .verify_is_valid_for_subject_name(server_name)
            .map_err(|_| fail())?;
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, signature, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn origin_and_host(url: &str) -> Result<(String, String), String> {
    let parsed = url::Url::parse(url).map_err(|_| "Invalid TLS pin URL")?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("TLS pins require an HTTPS origin without credentials".into());
    }
    let host = parsed
        .host_str()
        .ok_or("TLS pin URL has no host")?
        .to_owned();
    Ok((parsed.origin().ascii_serialization(), host))
}

fn fingerprint(value: &str) -> Result<[u8; 32], String> {
    let value = value.replace(':', "");
    if value.len() != 64 || !value.bytes().all(|x| x.is_ascii_hexdigit()) {
        return Err(
            "TLS pin must be a SHA256 certificate fingerprint (64 hexadecimal characters)".into(),
        );
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|_| "Invalid TLS pin")?;
    }
    Ok(out)
}

fn build_client(
    host: String,
    pin: [u8; 32],
    proxy: Option<&str>,
) -> Result<reqwest::Client, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedVerifier {
        host,
        fingerprint: pin,
        algorithms: provider.signature_verification_algorithms,
    });
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    super::http_client::build_client_from(
        super::http_client::base_builder()
            .use_preconfigured_tls(tls)
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none()),
        proxy,
    )
}

/// Exact origin lookup; changing/removing a pin takes effect on the next request.
/// Connections are pooled only for the same origin, fingerprint and proxy.
pub(crate) fn client_for_url(url: &str) -> Result<Option<reqwest::Client>, String> {
    let path = crate::config::get_app_config_dir().join("tls-pins.json");
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Cannot read tls-pins.json: {e}")),
    };
    if raw.len() > 65536 {
        return Err("tls-pins.json exceeds 64 KiB".into());
    }
    let pins: HashMap<String, String> = serde_json::from_slice(&raw).map_err(|_| {
        "Invalid tls-pins.json: expected HTTPS origins mapped to SHA256 fingerprints"
    })?;
    // HTTP and other schemes never use a TLS override.
    if !url.starts_with("https://") {
        return Ok(None);
    }
    let (origin, host) = origin_and_host(url)?;
    let Some(value) = pins.get(&origin) else {
        return Ok(None);
    };
    let pin = fingerprint(value)?;
    let proxy = super::http_client::proxy_url_for(url);
    static CACHE: OnceLock<Mutex<HashMap<String, reqwest::Client>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = format!("{origin}|{value}|{}", proxy.as_deref().unwrap_or(""));
    let mut cache = cache
        .lock()
        .map_err(|_| "TLS pin client cache lock poisoned")?;
    if let Some(client) = cache.get(&key) {
        return Ok(Some(client.clone()));
    }
    let client = build_client(host, pin, proxy.as_deref())?;
    if cache.len() >= 16 {
        cache.clear();
    }
    cache.insert(key, client.clone());
    Ok(Some(client))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_origins_include_scheme_host_and_port() {
        assert_eq!(
            origin_and_host("https://example.com:21137/v1/models")
                .unwrap()
                .0,
            "https://example.com:21137"
        );
        assert_ne!(
            origin_and_host("https://example.com/v1").unwrap().0,
            "https://example.com:21137"
        );
        assert_ne!(
            origin_and_host("https://sub.example.com:21137/v1")
                .unwrap()
                .0,
            "https://example.com:21137"
        );
        assert!(origin_and_host("http://example.com").is_err());
        assert!(origin_and_host("https://user:password@example.com").is_err());
        assert!(fingerprint(&"gg".repeat(32)).is_err());
        assert!(fingerprint("00").is_err());
        assert_eq!(fingerprint(&"AB:".repeat(32)).unwrap(), [0xab; 32]);
    }

    async fn tls_fixture(
        expired: bool,
    ) -> (
        String,
        CertificateDer<'static>,
        tokio::task::JoinHandle<bool>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        if expired {
            params.not_after = rcgen::date_time_ymd(2020, 1, 1);
        }
        let cert = params.self_signed(&key).unwrap().der().clone();
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let Ok(mut tls) = acceptor.accept(socket).await else {
                return false;
            };
            let mut buffer = [0; 2048];
            if tls.read(&mut buffer).await.unwrap_or(0) == 0 {
                return false;
            }
            tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
            let _ = tls.shutdown().await;
            true
        });
        (format!("https://localhost:{port}"), cert, task)
    }

    #[tokio::test]
    async fn pin_accepts_only_matching_unexpired_certificate_and_hostname() {
        for (wrong_pin, expired, wrong_name) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let (url, cert, task) = tls_fixture(expired).await;
            let pin = if wrong_pin {
                [0; 32]
            } else {
                Sha256::digest(cert.as_ref()).into()
            };
            let host = if wrong_name {
                "different.example"
            } else {
                "localhost"
            };
            let client = build_client(host.into(), pin, None).unwrap();
            let response = client
                .get(&url)
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await;
            let expected = !wrong_pin && !expired && !wrong_name;
            assert_eq!(
                response.is_ok(),
                expected,
                "pin={wrong_pin},expired={expired},name={wrong_name}"
            );
            if let Ok(response) = response {
                assert_eq!(response.text().await.unwrap(), "{}");
            }
            assert_eq!(
                task.await.unwrap(),
                expected,
                "Rejected TLS must not send an HTTP request"
            );
        }
    }

    /// Explicit opt-in, metadata GETs only. Never calls a generation endpoint.
    #[tokio::test]
    #[ignore]
    async fn configured_pin_metadata_get_probe() {
        let base = std::env::var("CC_SWITCH_TLS_PROBE_ORIGIN").unwrap();
        let token = std::env::var("CC_SWITCH_TLS_PROBE_TOKEN").unwrap();
        let client = client_for_url(&base)
            .unwrap()
            .expect("Explicit pin required");
        for path in ["/v1/models", "/v1/usage", "/api/v1/usage"] {
            let response = client
                .get(format!("{base}{path}"))
                .bearer_auth(&token)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .unwrap();
            let status = response.status().as_u16();
            let body = response.text().await.unwrap();
            let body = serde_json::from_str::<serde_json::Value>(&body)
                .unwrap_or_else(|_| serde_json::json!({"non_json": true}));
            println!(
                "PIN_METADATA {}",
                serde_json::json!({"path": path, "status": status, "body": body})
            );
        }
    }
}
