use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, HeaderValue, Method};
use http::header::{DATE, HOST, HeaderName};
use httpdate::parse_http_date;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use rsa::{RsaPrivateKey, RsaPublicKey};
use rustodon::mastodon::{
    HttpSignatureError, HttpSignatureKey, HttpSignatureRequest, HttpSignatureSigner,
    body_digest_header, sign_http_signature, sign_http_signature_with_headers,
    verify_http_signature,
};

const KEY_ID: &str = "https://remote.domain/users/bob#main-key";
const DATE_VALUE: &str = "Wed, 20 Dec 2023 10:00:00 GMT";
const GET_SIGNATURE: &str = concat!(
    r#"keyId="https://remote.domain/users/bob#main-key",algorithm="rsa-sha256",headers="date host (request-target)",signature=""#,
    "Z8ilar3J7bOwqZkMp7sL8sRs4B1FT+UorbmvWoE+A5UeoOJ3KBcUmbsh+k3wQwbP5gMNUrra9rEWabpasZGphLsbDxfbsWL3Cf0PllAc7c1c7AFEwnewtExI83/qqgEkfWc2z7UDutXc2NfgAx89Ox8DXU/fA2GG0jILjB6UpFyNugkY9rg6oI31UnvfVi3R7sr3/x8Ea3I9thPvqI2byF6cojknSpDAwYzeKdngX3TAQEGzFHz3SDWwyp3jeMWfwvVVbM38FxhvAnSumw7YwWW4L7M7h4M68isLimoT3yfCn2ucBVL5Dz8koBpYf/40w7QidClAwCafZQFC29yDOg==",
    r#"""#
);
const POST_SIGNATURE: &str = concat!(
    r#"keyId="https://remote.domain/users/bob#main-key",algorithm="rsa-sha256",headers="host date digest (request-target)",signature=""#,
    "gmhMjgMROGElJU3fpehV2acD5kMHeELi8EFP2UPHOdQ54H0r55AxIpji+J3lPe+N2qSb/4H1KXIh6f0lRu8TGSsu12OQmg5hiO8VA9flcA/mh9Lpk+qwlQZIPRqKP9xUEfqD+Z7ti5wPzDKrWAUK/7FIqWgcT/mlqB1R1MGkpMFc/q4CIs2OSNiWgA4K+Kp21oQxzC2kUuYob04gAZ7cyE/FTia5t08uv6lVYFdRsn4XNPn1MsHgFBwBMRG79ng3SyhoG4PrqBEi5q2IdLq3zfre/M6He3wlCpyO2VJNdGVoTIzeZ0Zz8jUscPV3XtWUchpGclLGSaKaq/JyNZeiYQ==",
    r#"""#
);

fn private_key() -> &'static str {
    static PRIVATE_KEY: OnceLock<String> = OnceLock::new();
    PRIVATE_KEY
        .get_or_init(|| {
            let source = include_str!(
                "../target/mastodon-v4.6.5/spec/requests/signature_verification_spec.rb"
            );
            let start = source
                .find("-----BEGIN RSA PRIVATE KEY-----")
                .expect("Mastodon test key should have a PEM header");
            let end = source[start..]
                .find("-----END RSA PRIVATE KEY-----")
                .map(|offset| start + offset + "-----END RSA PRIVATE KEY-----".len())
                .expect("Mastodon test key should have a PEM footer");
            format!(
                "{}\n",
                source[start..end]
                    .lines()
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })
        .as_str()
}

fn public_key() -> String {
    let private = RsaPrivateKey::from_pkcs1_pem(private_key()).expect("test RSA key should parse");
    RsaPublicKey::from(&private)
        .to_public_key_pem(LineEnding::LF)
        .expect("test RSA public key should encode")
}

fn now() -> SystemTime {
    parse_http_date(DATE_VALUE).expect("test date should parse")
}

fn request<'a>(
    method: &'a Method,
    path: &'a str,
    headers: &'a HeaderMap,
    body: &'a [u8],
) -> HttpSignatureRequest<'a> {
    HttpSignatureRequest::new(method, path, headers, body)
}

fn get_request<'a>(headers: &'a HeaderMap, path: &'a str) -> HttpSignatureRequest<'a> {
    request(&Method::GET, path, headers, &[])
}

fn post_request<'a>(headers: &'a HeaderMap, body: &'a [u8]) -> HttpSignatureRequest<'a> {
    request(&Method::POST, "/activitypub/success", headers, body)
}

fn get_headers(signature: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("www.example.com"));
    headers.insert(DATE, HeaderValue::from_static(DATE_VALUE));
    if let Some(signature) = signature {
        headers.insert(
            HeaderName::from_static("signature"),
            HeaderValue::from_str(signature).expect("test signature should be a valid header"),
        );
    }
    headers
}

fn key(public_key_pem: &str) -> HttpSignatureKey<'_> {
    HttpSignatureKey {
        key_id: KEY_ID,
        public_key_pem,
    }
}

fn signer() -> HttpSignatureSigner<'static> {
    HttpSignatureSigner {
        key_id: KEY_ID,
        private_key_pem: private_key(),
    }
}

#[test]
fn verifies_and_reproduces_mastodon_get_vector() {
    let headers = get_headers(Some(GET_SIGNATURE));
    let request = get_request(&headers, "/activitypub/success");
    let public_key_pem = public_key();

    let verified = verify_http_signature(&request, &key(&public_key_pem), now())
        .expect("Mastodon GET vector should verify");
    assert_eq!(verified.key_id, KEY_ID);
    assert_eq!(
        verified.signed_headers,
        ["date", "host", "(request-target)"]
    );

    let signature = sign_http_signature_with_headers(
        &request,
        &signer(),
        &["date", "host", "(request-target)"],
    )
    .expect("Mastodon GET vector should sign");
    assert_eq!(signature, GET_SIGNATURE);
}

#[test]
fn verifies_query_string_and_mastodons_legacy_queryless_fallback() {
    let public_key_pem = public_key();
    let full_headers = get_headers(Some(GET_SIGNATURE));
    let request = get_request(&full_headers, "/activitypub/success?foo=42");
    verify_http_signature(&request, &key(&public_key_pem), now())
        .expect("Mastodon should accept the legacy queryless signature");

    let query_path_headers = get_headers(None);
    let query_path = get_request(&query_path_headers, "/activitypub/success?foo=42");
    let query_signature = sign_http_signature_with_headers(
        &query_path,
        &signer(),
        &["date", "host", "(request-target)"],
    )
    .expect("query vector should sign");
    let query_headers = get_headers(Some(&query_signature));
    verify_http_signature(
        &get_request(&query_headers, "/activitypub/success?foo=42"),
        &key(&public_key_pem),
        now(),
    )
    .expect("query signature should verify");
    assert_eq!(
        verify_http_signature(
            &get_request(&query_headers, "/activitypub/success?foo=43"),
            &key(&public_key_pem),
            now(),
        ),
        Err(HttpSignatureError::SignatureMismatch)
    );

    let path_headers = get_headers(None);
    let signed_path = get_request(&path_headers, "/activitypub/success");
    let signature = sign_http_signature_with_headers(
        &signed_path,
        &signer(),
        &["date", "host", "(request-target)"],
    )
    .expect("path-only vector should sign");
    let query_headers = get_headers(Some(&signature));
    let request = get_request(&query_headers, "/activitypub/success?foo=42");
    verify_http_signature(&request, &key(&public_key_pem), now())
        .expect("legacy queryless signature should verify");
}

#[test]
fn signs_and_verifies_post_body_digest() {
    let body = b"Hello world";
    let digest = body_digest_header(body);
    assert_eq!(
        digest,
        "SHA-256=ZOyIygCyaOW6GjVnihtTFtIS9PNmskdyMlNKiuyjfzw="
    );

    let mut headers = get_headers(None);
    headers.insert(
        HeaderName::from_static("digest"),
        HeaderValue::from_str(&digest).expect("digest should be a valid header"),
    );
    let mut fixed_headers = headers.clone();
    fixed_headers.insert(
        HeaderName::from_static("signature"),
        HeaderValue::from_static(POST_SIGNATURE),
    );
    verify_http_signature(
        &post_request(&fixed_headers, body),
        &key(&public_key()),
        now(),
    )
    .expect("Mastodon POST vector should verify");

    assert_eq!(
        sign_http_signature(&post_request(&headers, body), &signer())
            .expect("Mastodon POST vector should sign"),
        POST_SIGNATURE
    );
    let signature =
        sign_http_signature(&post_request(&headers, body), &signer()).expect("POST should sign");
    headers.insert(
        HeaderName::from_static("signature"),
        HeaderValue::from_str(&signature).expect("signature should be a valid header"),
    );
    let public_key_pem = public_key();

    verify_http_signature(&post_request(&headers, body), &key(&public_key_pem), now())
        .expect("POST should verify");
}

#[test]
fn rejects_missing_strength_headers_bad_digest_and_stale_dates() {
    let public_key_pem = public_key();
    let signer = signer();
    let mut headers = get_headers(None);
    let signature = sign_http_signature_with_headers(
        &get_request(&headers, "/activitypub/success"),
        &signer,
        &["date", "(request-target)"],
    )
    .expect("weak GET signature should still be constructible");
    headers.insert(
        HeaderName::from_static("signature"),
        HeaderValue::from_str(&signature).unwrap(),
    );
    headers.remove(HOST);
    assert_eq!(
        verify_http_signature(
            &get_request(&headers, "/activitypub/success"),
            &key(&public_key_pem),
            now(),
        ),
        Err(HttpSignatureError::MissingHost)
    );

    let body = b"Hello world";
    let mut post_headers = get_headers(None);
    post_headers.insert(
        HeaderName::from_static("digest"),
        HeaderValue::from_static("SHA-256=invalid"),
    );
    let signature = sign_http_signature_with_headers(
        &post_request(&post_headers, body),
        &signer,
        &["host", "date", "(request-target)"],
    )
    .expect("weak POST signature should still be constructible");
    post_headers.insert(
        HeaderName::from_static("signature"),
        HeaderValue::from_str(&signature).unwrap(),
    );
    assert_eq!(
        verify_http_signature(
            &post_request(&post_headers, body),
            &key(&public_key_pem),
            now(),
        ),
        Err(HttpSignatureError::MissingDigest)
    );

    let mut tampered_headers = get_headers(None);
    tampered_headers.insert(
        HeaderName::from_static("digest"),
        HeaderValue::from_str(&body_digest_header(body)).unwrap(),
    );
    let signature = sign_http_signature(&post_request(&tampered_headers, body), &signer)
        .expect("valid POST should sign");
    tampered_headers.insert(
        HeaderName::from_static("signature"),
        HeaderValue::from_str(&signature).unwrap(),
    );
    assert_eq!(
        verify_http_signature(
            &post_request(&tampered_headers, b"Hello world!"),
            &key(&public_key_pem),
            now(),
        ),
        Err(HttpSignatureError::DigestMismatch)
    );

    let valid_headers = get_headers(Some(GET_SIGNATURE));
    assert_eq!(
        verify_http_signature(
            &get_request(&valid_headers, "/activitypub/success"),
            &key(&public_key_pem),
            now() + Duration::from_hours(13),
        ),
        Err(HttpSignatureError::OutsideTimeWindow)
    );

    let expires = now()
        .duration_since(UNIX_EPOCH)
        .expect("test date should be after the epoch")
        .as_secs()
        + 1;
    let expired_signature = format!("{GET_SIGNATURE},expires=\"{expires}\"");
    let expired_headers = get_headers(Some(&expired_signature));
    assert_eq!(
        verify_http_signature(
            &get_request(&expired_headers, "/activitypub/success"),
            &key(&public_key_pem),
            now() + Duration::from_hours(1) + Duration::from_secs(2),
        ),
        Err(HttpSignatureError::OutsideTimeWindow)
    );
}

#[test]
fn binds_signature_to_expected_key_and_rejects_newer_message_signatures() {
    let headers = get_headers(Some(GET_SIGNATURE));
    let request = get_request(&headers, "/activitypub/success");
    let public_key_pem = public_key();
    let wrong_key = HttpSignatureKey {
        key_id: "https://remote.domain/users/alice#main-key",
        public_key_pem: &public_key_pem,
    };
    assert_eq!(
        verify_http_signature(&request, &wrong_key, now()),
        Err(HttpSignatureError::KeyIdMismatch)
    );

    let mut message_headers = get_headers(Some(GET_SIGNATURE));
    message_headers.insert(
        HeaderName::from_static("signature-input"),
        HeaderValue::from_static("sig1=(\"@method\");created=1703066400"),
    );
    let message_request = get_request(&message_headers, "/activitypub/success");
    assert_eq!(
        verify_http_signature(&message_request, &key(&public_key_pem), now()),
        Err(HttpSignatureError::UnsupportedMessageSignature)
    );
}

#[test]
fn parses_legacy_spacing_and_rejects_duplicate_parameters() {
    let public_key_pem = public_key();
    let spaced = GET_SIGNATURE.replacen("keyId=", "keyId = ", 1);
    let headers = get_headers(Some(&spaced));
    verify_http_signature(
        &get_request(&headers, "/activitypub/success"),
        &key(&public_key_pem),
        now(),
    )
    .expect("Rails-compatible spacing should parse");

    let duplicate = format!("{GET_SIGNATURE},keyId=\"{KEY_ID}\"");
    let headers = get_headers(Some(&duplicate));
    assert_eq!(
        verify_http_signature(
            &get_request(&headers, "/activitypub/success"),
            &key(&public_key_pem),
            now(),
        ),
        Err(HttpSignatureError::DuplicateSignatureParameter)
    );
}

#[test]
fn signer_debug_output_redacts_private_key_material() {
    let rendered = format!("{:?}", signer());

    assert!(!rendered.contains("BEGIN RSA PRIVATE KEY"));
    assert!(rendered.contains("REDACTED"));
}
