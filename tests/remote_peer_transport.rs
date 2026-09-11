//! Real TLS transport tests; env is passed to subprocesses, never mutated in the test runner.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use rustodon::mastodon::{
    HttpSignatureKey, HttpSignatureRequest, HttpSignatureSigner, verify_http_signature,
};
use rustodon::remote::{
    RemoteAccountResolver, RemoteDomainBudget, RemoteFetchError, RemoteFetchLimits, RemoteFetcher,
};
use url::Url;

const ORIGIN: &str = "https://peer.invalid:19443";
const ORIGINS_ENV: &str = "RUSTODON_TEST_PEER_ORIGINS";
const CA_ENV: &str = "RUSTODON_TEST_PEER_CA";

fn child(mode: &str, origins: Option<&str>, ca: Option<&Path>) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "peer_transport_child", "--nocapture"])
        .env("PEER_TRANSPORT_CHILD", mode)
        .env_remove(ORIGINS_ENV)
        .env_remove(CA_ENV);
    if let Some(origins) = origins {
        command.env(ORIGINS_ENV, origins);
    }
    if let Some(ca) = ca {
        command.env(CA_ENV, ca);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{mode}, {origins:?}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn certificate(directory: &Path, name: &str) {
    let output = Command::new("openssl")
        .current_dir(directory)
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=peer.invalid",
            "-addext",
            "subjectAltName=DNS:peer.invalid",
            "-keyout",
            &format!("{name}.key"),
            "-out",
            &format!("{name}.pem"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn peer_transport_environment_and_tls() {
    let directory =
        std::env::temp_dir().join(format!("rustodon-peer-transport-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    certificate(&directory, "ca");
    certificate(&directory, "untrusted");
    for args in [
        vec![
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-subj",
            "/CN=peer.invalid",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
        ],
        vec![
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-days",
            "2",
            "-extfile",
            "server.ext",
            "-out",
            "server.pem",
        ],
    ] {
        std::fs::write(directory.join("server.ext"), "subjectAltName=DNS:peer.invalid\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n").unwrap();
        let output = Command::new("openssl")
            .current_dir(&directory)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let ca = directory.join("ca.pem");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = listener.local_addr().unwrap();
    let map = format!(r#"{{"{ORIGIN}":"{endpoint}","https://wrong.invalid":"{endpoint}"}}"#);
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from_pem_file(directory.join("server.pem")).unwrap()],
            PrivateKeyDer::from_pem_file(directory.join("server.key")).unwrap(),
        )
        .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        let config = Arc::new(config);
        while !server_stop.load(Ordering::Relaxed) {
            let Ok((socket, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let connection = rustls::ServerConnection::new(Arc::clone(&config)).unwrap();
            let mut stream = rustls::StreamOwned::new(connection, socket);
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    break;
                }
                request.push(byte[0]);
            }
            if !request.ends_with(b"\r\n\r\n") {
                continue;
            } // Expected failed TLS handshakes.
            let request = String::from_utf8(request).unwrap();
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("\r\nhost: peer.invalid:19443\r\n"),
                "{request}"
            );
            assert_eq!(stream.conn.server_name(), Some("peer.invalid"));
            let headers: http::HeaderMap = request
                .lines()
                .skip(1)
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| {
                    (
                        name.parse::<http::HeaderName>().unwrap(),
                        value.trim().parse::<http::HeaderValue>().unwrap(),
                    )
                })
                .collect();
            let length = headers
                .get("content-length")
                .map_or(0, |value| value.to_str().unwrap().parse::<usize>().unwrap());
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let path = request.split_whitespace().nth(1).unwrap();
            if headers.contains_key("signature") {
                let private = rsa::RsaPrivateKey::from_pkcs1_pem(include_str!(
                    "fixtures/http-signature-private.pem"
                ))
                .unwrap();
                let public = rsa::RsaPublicKey::from(&private)
                    .to_public_key_pem(LineEnding::LF)
                    .unwrap();
                let method = request
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse::<http::Method>()
                    .unwrap();
                verify_http_signature(
                    &HttpSignatureRequest::new(&method, path, &headers, &body),
                    &HttpSignatureKey {
                        key_id: "https://sender.invalid/actor#key",
                        public_key_pem: &public,
                    },
                    std::time::SystemTime::now(),
                )
                .unwrap();
            }
            if path.starts_with("/signed") || request.starts_with("POST ") {
                assert!(
                    request.to_ascii_lowercase().contains("\r\nsignature: "),
                    "{request}"
                );
            }
            let (status, extra, body) = match path {
                "/redirect" => (
                    "307 Temporary Redirect",
                    "Location: /final?x=1\r\n",
                    String::new(),
                ),
                "/signed-redirect" => (
                    "307 Temporary Redirect",
                    "Location: /signed-final?x=1\r\n",
                    String::new(),
                ),
                "/escape" => (
                    "307 Temporary Redirect",
                    "Location: https://unmapped.invalid/inbox\r\n",
                    String::new(),
                ),
                "/loop" => (
                    "307 Temporary Redirect",
                    "Location: /loop\r\n",
                    String::new(),
                ),
                "/actor" => (
                    "200 OK",
                    "",
                    format!(
                        r#"{{"@context":"https://www.w3.org/ns/activitystreams","id":"{ORIGIN}/actor","type":"Person","preferredUsername":"alice","inbox":"{ORIGIN}/inbox","url":"{ORIGIN}/profile"}}"#
                    ),
                ),
                _ if path.starts_with("/.well-known/webfinger?") => (
                    "200 OK",
                    "",
                    format!(
                        r#"{{"subject":"acct:alice@peer.invalid:19443","links":[{{"rel":"self","type":"application/activity+json","href":"{ORIGIN}/actor"}}]}}"#
                    ),
                ),
                _ => ("200 OK", "", "{}".to_owned()),
            };
            let content_type = if path.starts_with("/.well-known/") {
                "application/jrd+json"
            } else {
                "application/activity+json"
            };
            let response = format!(
                "HTTP/1.1 {status}\r\n{extra}Content-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    // This is also a negative capability test when built without the double gate.
    child("valid", Some(&map), Some(&ca));
    if cfg!(all(debug_assertions, feature = "test-support")) {
        child("network", Some(&map), Some(&ca));
        child(
            "tls-failure",
            Some(&map),
            Some(&directory.join("untrusted.pem")),
        );
        child("invalid", None, Some(&ca));
        child("invalid", Some(&map), None);
        child("invalid", Some(&map), Some(&directory.join("missing.pem")));
        std::fs::write(directory.join("bad.pem"), "not a certificate").unwrap();
        child("invalid", Some(&map), Some(&directory.join("bad.pem")));
        for pem in [
            String::new(),
            format!("{}junk", std::fs::read_to_string(&ca).unwrap()),
            "-----BEGIN CERTIFICATE-----\nYmFk\n-----END CERTIFICATE-----".to_owned(),
        ] {
            std::fs::write(directory.join("bad.pem"), pem).unwrap();
            child("invalid", Some(&map), Some(&directory.join("bad.pem")));
        }
        let partial_map =
            format!(r#"{{"{ORIGIN}":"{endpoint}","https://public.example":"127.0.0.1:443"}}"#);
        child("invalid", Some(&partial_map), Some(&ca));
        for invalid in [
            "",
            "{}",
            "[]",
            "null",
            "not json",
            r#"{"http://peer.invalid":"127.0.0.1:443"}"#,
            r#"{"https://peer.example":"127.0.0.1:443"}"#,
            r#"{"https://127.0.0.1":"127.0.0.1:443"}"#,
            r#"{"https://PEER.invalid":"127.0.0.1:443"}"#,
            r#"{"https://peer.invalid/":"127.0.0.1:443"}"#,
            r#"{"https://peer.invalid:443":"127.0.0.1:443"}"#,
            r#"{"https://peer.invalid/path":"127.0.0.1:443"}"#,
            r#"{"https://peer.invalid?x":"127.0.0.1:443"}"#,
            r#"{"https://peer.invalid#x":"127.0.0.1:443"}"#,
            r#"{"https://user@peer.invalid":"127.0.0.1:443"}"#,
            r#"{"https://peer.invalid":"8.8.8.8:443"}"#,
            r#"{"https://peer.invalid":"10.0.0.1:443"}"#,
            r#"{"https://peer.invalid":"0.0.0.0:443"}"#,
            r#"{"https://peer.invalid":"127.0.0.1:0"}"#,
            r#"{"https://peer.invalid":"localhost:443"}"#,
            r#"{"https://peer.invalid":"127.0.0.1:443","https://peer.invalid":"127.0.0.1:444"}"#,
        ] {
            child("invalid", Some(invalid), Some(&ca));
        }
        child(
            "valid",
            Some(r#"{"https://peer.invalid:19443":"[::1]:443"}"#),
            Some(&ca),
        );
    }
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn peer_transport_child() {
    let Ok(mode) = std::env::var("PEER_TRANSPORT_CHILD") else {
        return;
    };
    let url = |path: &str| Url::parse(&format!("{ORIGIN}{path}")).unwrap();
    let limits = RemoteFetchLimits {
        request_timeout: Duration::from_secs(3),
        ..RemoteFetchLimits::default()
    };
    let fetcher = RemoteFetcher::new(limits);
    if mode == "valid" {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let fetchers = [
            RemoteFetcher::default(),
            fetcher.clone(),
            RemoteFetcher::with_domain_budget(limits, RemoteDomainBudget::default()),
            fetcher.with_limits(limits),
        ];
        for fetcher in fetchers {
            let result = fetcher.validate_target(&url("/inbox")).await;
            assert_eq!(
                result.is_ok(),
                cfg!(all(debug_assertions, feature = "test-support")),
                "{result:?}"
            );
        }
        // Pool-derived fetchers must still reject unmapped targets without needing a connection.
        if cfg!(all(debug_assertions, feature = "test-support")) {
            assert!(matches!(
                fetcher
                    .with_operational_pool(pool)
                    .validate_target(&Url::parse("https://unmapped.invalid/inbox").unwrap())
                    .await,
                Err(RemoteFetchError::InvalidUrl)
            ));
        }
        return;
    }
    if mode == "invalid" {
        let result = fetcher.validate_target(&url("/inbox")).await;
        assert!(
            matches!(result, Err(RemoteFetchError::Client)),
            "{result:?}"
        );
        assert!(matches!(
            fetcher.get(url("/"), &[]).await,
            Err(RemoteFetchError::Client)
        ));
        return;
    }
    if mode == "tls-failure" {
        assert!(matches!(
            fetcher.get(url("/"), &[]).await,
            Err(RemoteFetchError::Request)
        ));
        return;
    }
    let signer = HttpSignatureSigner {
        key_id: "https://sender.invalid/actor#key",
        private_key_pem: include_str!("fixtures/http-signature-private.pem"),
    };
    for path in ["/", "/redirect", "/media"] {
        let response = fetcher.get(url(path), &[]).await.unwrap();
        assert_eq!(response.body, b"{}");
        assert_eq!(response.url.host_str(), Some("peer.invalid"));
        assert_eq!(response.url.port(), Some(19443));
    }
    fetcher
        .get_signed(url("/signed-redirect"), &[], &signer)
        .await
        .unwrap();
    fetcher
        .post_signed_json(url("/redirect"), b"{}", &signer)
        .await
        .unwrap();
    let actor = RemoteAccountResolver::new(RemoteFetcher::default())
        .resolve("alice", "peer.invalid:19443")
        .await
        .unwrap();
    assert_eq!(actor.id, url("/actor"));
    for destination in [
        "https://unmapped.invalid/",
        "https://peer.invalid/",
        "http://peer.invalid:19443/",
        "https://127.0.0.1/",
        "https://example.com/",
    ] {
        let target = Url::parse(destination).unwrap();
        assert!(matches!(
            fetcher.validate_target(&target).await,
            Err(RemoteFetchError::InvalidUrl)
        ));
        assert!(matches!(
            fetcher.get(target, &[]).await,
            Err(RemoteFetchError::InvalidUrl)
        ));
    }
    assert!(matches!(
        fetcher
            .get(Url::parse("https://wrong.invalid/").unwrap(), &[])
            .await,
        Err(RemoteFetchError::Request)
    ));
    assert!(matches!(
        fetcher.get(url("/escape"), &[]).await,
        Err(RemoteFetchError::InvalidUrl)
    ));
    assert!(matches!(
        fetcher.get_signed(url("/escape"), &[], &signer).await,
        Err(RemoteFetchError::OriginMismatch)
    ));
    assert!(matches!(
        fetcher
            .post_signed_json(url("/escape"), b"{}", &signer)
            .await,
        Err(RemoteFetchError::OriginMismatch)
    ));
    assert!(matches!(
        fetcher.get(url("/loop"), &[]).await,
        Err(RemoteFetchError::TooManyRedirects)
    ));
    let tiny = fetcher.with_limits(RemoteFetchLimits {
        max_response_bytes: 1,
        max_request_bytes: 1,
        ..limits
    });
    assert!(matches!(
        tiny.get(url("/"), &[]).await,
        Err(RemoteFetchError::BodyTooLarge)
    ));
    assert!(matches!(
        tiny.post_signed_json(url("/"), b"{}", &signer).await,
        Err(RemoteFetchError::BodyTooLarge)
    ));
}
