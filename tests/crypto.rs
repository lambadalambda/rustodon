use std::fmt::{Debug, Display};

use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPublicKey};
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::{RsaPrivateKey, RsaPublicKey};
use rustodon::crypto::{
    ActiveRecordDecryptionError, ActiveRecordEncryptionConfig, EncryptionConfigError, RsaKeyError,
    validate_rsa_signing_keypair,
};
use rustodon::secret::SecretString;

const PRIMARY_KEY: &str = "33333333333333333333333333333333";
const DETERMINISTIC_KEY: &str = "11111111111111111111111111111111";
const DERIVATION_SALT: &str = "22222222222222222222222222222222";
const FIXTURE_PLAINTEXT: &str = "fixture opaque private key material";
const FIXTURE_ENVELOPE: &str = r#"{"p":"9q4gHslWnbK8a5zNjL1ySdpX5I8wunpM6ed7whTvAnwlyUs=","h":{"iv":"jvVW4mOA+IzCBTv/","at":"Wd+qB8DnQnN2bRo/7ukJmQ=="}}"#;
const SHA1_ENVELOPE: &str = r#"{"p":"vnJL0+zip9Yw36jvDFqgjnFfPQ==","h":{"iv":"OA4gNMbSQDexohIz","at":"UTeomDwdH/tfTjN4fyBalA=="}}"#;
const COMPRESSED_ENVELOPE: &str = r#"{"p":"H/bWMr+TIYQ4QFoe18y6TLnia8HrsOgEQMcTl9UlDY2CPAEk8Ubx","h":{"iv":"QqOYCK9n7NEPMknB","at":"peaKTlLzZfgHrzgVIC/31A==","c":true}}"#;
const INVALID_UTF8_ENVELOPE: &str =
    r#"{"p":"pg==","h":{"iv":"HfzUZAtICvcxzF3y","at":"73Er8oK18Pbv6SJJQlCnrw==","e":"VVRGLTg="}}"#;
const SEED_SQL: &str = include_str!("../fixtures/mastodon/v4.6.5/seed.sql");

fn secret(value: &str) -> SecretString {
    SecretString::new(value.to_owned())
}

fn encryption_config() -> ActiveRecordEncryptionConfig {
    ActiveRecordEncryptionConfig::new(
        secret(PRIMARY_KEY),
        secret(DETERMINISTIC_KEY),
        secret(DERIVATION_SALT),
    )
    .expect("fixture encryption configuration should be valid")
}

fn assert_safe_error(error: &(impl Debug + Display), forbidden: &[&str]) {
    let rendered = format!("{error:?} {error}");
    for value in forbidden {
        assert!(
            !rendered.contains(value),
            "error output exposed sensitive input"
        );
    }
}

fn fixture_literal(delimiter: &str) -> &'static str {
    let (_, remainder) = SEED_SQL
        .split_once(delimiter)
        .expect("fixture delimiter should be present");
    let (value, _) = remainder
        .split_once(delimiter)
        .expect("fixture delimiter should be paired");
    value
}

fn fixture_rsa_pems() -> (&'static str, &'static str) {
    let private_key = fixture_literal("$fixture_private$");
    let public_key = fixture_literal("$fixture_public$");
    assert!(private_key.starts_with("-----BEGIN RSA PRIVATE KEY-----\nMIIEowIB"));
    assert!(private_key.ends_with("-----END RSA PRIVATE KEY-----\n"));
    assert!(public_key.starts_with("-----BEGIN PUBLIC KEY-----\nMIIBIjAN"));
    assert!(public_key.ends_with("-----END PUBLIC KEY-----\n"));
    (private_key, public_key)
}

#[test]
fn decrypts_checked_mastodon_rails_fixture() {
    assert!(SEED_SQL.contains(FIXTURE_ENVELOPE));
    let plaintext = encryption_config()
        .decrypt_string(FIXTURE_ENVELOPE, 4_096)
        .expect("checked Mastodon fixture should decrypt");

    assert_eq!(plaintext.expose_secret(), FIXTURE_PLAINTEXT);
    assert_eq!(format!("{plaintext:?}"), "SecretString([REDACTED])");
    assert_eq!(plaintext.to_string(), "[REDACTED]");
}

#[test]
fn encrypts_a_secret_that_the_rails_decryptor_can_read() {
    let encrypted = encryption_config()
        .encrypt_string("fixture TOTP secret")
        .expect("fixture secret should encrypt");
    assert_ne!(encrypted, "fixture TOTP secret");
    assert_eq!(
        encryption_config()
            .decrypt_string(&encrypted, 4_096)
            .expect("Rust-generated envelope should decrypt")
            .expose_secret(),
        "fixture TOTP secret"
    );
}

#[test]
fn sha1_is_supported_for_legacy_non_deterministic_ciphertext() {
    let plaintext = encryption_config()
        .decrypt_string(SHA1_ENVELOPE, 4_096)
        .expect("legacy Rails SHA-1-derived ciphertext should decrypt");

    assert_eq!(plaintext.expose_secret(), "legacy sha1 fixture");
}

#[test]
fn decrypts_rails_zlib_payload_with_a_bound() {
    let plaintext = encryption_config()
        .decrypt_string(COMPRESSED_ENVELOPE, 1_024)
        .expect("Rails zlib fixture should decrypt");

    assert_eq!(
        plaintext.expose_secret(),
        "compressed Mastodon fixture ".repeat(8)
    );
    assert_eq!(
        encryption_config().decrypt_string(COMPRESSED_ENVELOPE, 32),
        Err(ActiveRecordDecryptionError::PlaintextTooLarge)
    );
}

#[test]
fn configuration_requires_all_three_secrets_without_exposing_them() {
    let error =
        ActiveRecordEncryptionConfig::new(secret(PRIMARY_KEY), secret(""), secret(DERIVATION_SALT))
            .expect_err("the deterministic key is required even for non-deterministic keypairs");

    assert_eq!(error, EncryptionConfigError::MissingDeterministicKey);
    assert_safe_error(&error, &[PRIMARY_KEY, DETERMINISTIC_KEY, DERIVATION_SALT]);
    assert_safe_error(
        &encryption_config(),
        &[PRIMARY_KEY, DETERMINISTIC_KEY, DERIVATION_SALT],
    );
}

#[test]
fn wrong_secret_reports_only_authentication_failure() {
    let config = ActiveRecordEncryptionConfig::new(
        secret("wrong primary key must remain private"),
        secret(DETERMINISTIC_KEY),
        secret(DERIVATION_SALT),
    )
    .expect("non-empty test configuration should be valid");
    let error = config
        .decrypt_string(FIXTURE_ENVELOPE, 4_096)
        .expect_err("a wrong primary key must fail authentication");

    assert_eq!(error, ActiveRecordDecryptionError::AuthenticationFailed);
    assert_safe_error(
        &error,
        &[
            "wrong primary key must remain private",
            FIXTURE_ENVELOPE,
            FIXTURE_PLAINTEXT,
        ],
    );
}

#[test]
fn rejects_non_compact_or_malformed_envelopes() {
    let malformed = [
        "not-json",
        "{}",
        r#"{ "p":"","h":{"iv":"","at":""}}"#,
        r#"{"p":"","h":{"iv":"","at":""},"extra":true}"#,
        r#"{"p":"","h":{"iv":"","at":"","extra":true}}"#,
        r#"{"p":"","p":"","h":{"iv":"","at":""}}"#,
        r#"{"p":"","h":{"iv":"","iv":"","at":""}}"#,
        r#"{"p":"","h":{"iv":"","at":"","c":false}}"#,
    ];

    for envelope in malformed {
        assert_eq!(
            encryption_config().decrypt_string(envelope, 4_096),
            Err(ActiveRecordDecryptionError::InvalidEnvelope),
            "unexpected failure class for malformed envelope"
        );
    }
}

#[test]
fn rejects_non_canonical_standard_base64() {
    let malformed = [
        r#"{"p":"9q4gHslWnbK8a5zNjL1ySdpX5I8wunpM6ed7whTvAnwlyUs","h":{"iv":"jvVW4mOA+IzCBTv/","at":"Wd+qB8DnQnN2bRo/7ukJmQ=="}}"#,
        r#"{"p":"9q4gHslWnbK8a5zNjL1ySdpX5I8wunpM6ed7whTvAnwlyUs=","h":{"iv":"jvVW4mOA+IzCBTv_","at":"Wd+qB8DnQnN2bRo/7ukJmQ=="}}"#,
        r#"{"p":"%%%%","h":{"iv":"jvVW4mOA+IzCBTv/","at":"Wd+qB8DnQnN2bRo/7ukJmQ=="}}"#,
    ];

    for envelope in malformed {
        assert_eq!(
            encryption_config().decrypt_string(envelope, 4_096),
            Err(ActiveRecordDecryptionError::InvalidBase64)
        );
    }
}

#[test]
fn rejects_invalid_iv_and_authentication_tag_lengths() {
    let short_iv = r#"{"p":"","h":{"iv":"AAAAAAAAAAAAAAA=","at":"AAAAAAAAAAAAAAAAAAAAAA=="}}"#;
    let short_tag = r#"{"p":"","h":{"iv":"AAAAAAAAAAAAAAAA","at":"AAAAAAAAAAAAAAAAAAAA"}}"#;

    assert_eq!(
        encryption_config().decrypt_string(short_iv, 4_096),
        Err(ActiveRecordDecryptionError::InvalidIvLength)
    );
    assert_eq!(
        encryption_config().decrypt_string(short_tag, 4_096),
        Err(ActiveRecordDecryptionError::InvalidTagLength)
    );
}

#[test]
fn rejects_bad_tag_compression_encoding_and_plaintext() {
    let bad_tag = FIXTURE_ENVELOPE.replacen("Wd+q", "Xd+q", 1);
    let invalid_compression = FIXTURE_ENVELOPE.replacen("}}", r#","c":true}}"#, 1);
    let utf8_marker = FIXTURE_ENVELOPE.replacen("}}", r#","e":"VVRGLTg="}}"#, 1);
    let unsupported_encoding = FIXTURE_ENVELOPE.replacen("}}", r#","e":"QVNDSUktOEJJVA=="}}"#, 1);

    assert_eq!(
        encryption_config().decrypt_string(&bad_tag, 4_096),
        Err(ActiveRecordDecryptionError::AuthenticationFailed)
    );
    assert_eq!(
        encryption_config().decrypt_string(&invalid_compression, 4_096),
        Err(ActiveRecordDecryptionError::InvalidCompression)
    );
    assert_eq!(
        encryption_config()
            .decrypt_string(&utf8_marker, 4_096)
            .expect("an explicit UTF-8 marker should be accepted")
            .expose_secret(),
        FIXTURE_PLAINTEXT
    );
    assert_eq!(
        encryption_config().decrypt_string(&unsupported_encoding, 4_096),
        Err(ActiveRecordDecryptionError::UnsupportedEncoding)
    );
    assert_eq!(
        encryption_config().decrypt_string(INVALID_UTF8_ENVELOPE, 4_096),
        Err(ActiveRecordDecryptionError::InvalidPlaintextEncoding)
    );
}

#[test]
fn validates_published_fixture_rsa_keypair_and_supported_pem_containers() {
    let (private_pem, public_pem) = fixture_rsa_pems();
    validate_rsa_signing_keypair(Some(&secret(private_pem)), public_pem)
        .expect("published fixture RSA keypair should validate");

    let private = RsaPrivateKey::from_pkcs1_pem(private_pem)
        .expect("published fixture private key should parse");
    let private_pkcs8 = private
        .to_pkcs8_pem(LineEnding::LF)
        .expect("fixture private key should encode as PKCS#8");
    let public_pkcs1 = RsaPublicKey::from(&private)
        .to_pkcs1_pem(LineEnding::LF)
        .expect("fixture public key should encode as PKCS#1");

    validate_rsa_signing_keypair(Some(&secret(private_pkcs8.as_str())), public_pkcs1.as_str())
        .expect("PKCS#8 private and PKCS#1 public PEM should validate");
}

#[test]
fn rsa_validation_distinguishes_missing_corrupt_and_mismatched_keys() {
    let (private_pem, public_pem) = fixture_rsa_pems();
    let mismatched_public = public_pem.replacen("qIAYv", "qIAZv", 1);
    let corrupt_private = secret(concat!(
        "-----BEGIN RSA PRIVATE KEY-----\n",
        "private fixture bytes must not leak\n",
        "-----END RSA PRIVATE KEY-----\n",
    ));

    assert_eq!(
        validate_rsa_signing_keypair(None, public_pem),
        Err(RsaKeyError::MissingPrivateKey)
    );
    let corrupt_error = validate_rsa_signing_keypair(Some(&corrupt_private), public_pem)
        .expect_err("corrupt private PEM should fail");
    assert_eq!(corrupt_error, RsaKeyError::CorruptPrivateKey);
    assert_safe_error(
        &corrupt_error,
        &[corrupt_private.expose_secret(), public_pem],
    );

    let mismatch_error =
        validate_rsa_signing_keypair(Some(&secret(private_pem)), &mismatched_public)
            .expect_err("mismatched public key should fail");
    assert_eq!(mismatch_error, RsaKeyError::KeyMismatch);
    assert_safe_error(&mismatch_error, &[private_pem, mismatched_public.as_str()]);
}

#[test]
fn rsa_validation_rejects_missing_or_corrupt_public_pem_safely() {
    let (private_pem, _) = fixture_rsa_pems();
    let private_key = secret(private_pem);

    assert_eq!(
        validate_rsa_signing_keypair(Some(&private_key), ""),
        Err(RsaKeyError::MissingPublicKey)
    );
    let corrupt_public =
        "-----BEGIN PUBLIC KEY-----\npublic bytes must not leak\n-----END PUBLIC KEY-----";
    let error = validate_rsa_signing_keypair(Some(&private_key), corrupt_public)
        .expect_err("corrupt public PEM should fail");
    assert_eq!(error, RsaKeyError::CorruptPublicKey);
    assert_safe_error(&error, &[private_pem, corrupt_public]);
}
