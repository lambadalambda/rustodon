use std::error::Error;
use std::fmt;
use std::io::Read;

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::read::ZlibDecoder;
use pbkdf2::pbkdf2_hmac;
use rsa::pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey};
use rsa::pkcs1v15::{SigningKey, VerifyingKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::signature::{Signer, Verifier};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde_json::Value;
use sha1::Sha1;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::secret::SecretString;

const PBKDF2_ITERATIONS: u32 = 65_536;
const AES_KEY_LENGTH: usize = 32;
const GCM_IV_LENGTH: usize = 12;
const GCM_TAG_LENGTH: usize = 16;
const SIGNING_CHALLENGE: &[u8] = b"rustodon-preflight-signing-key-check-v1";

/// Required Rails Active Record encryption secrets.
pub struct ActiveRecordEncryptionConfig {
    primary_key: SecretString,
    deterministic_key: SecretString,
    key_derivation_salt: SecretString,
}

impl ActiveRecordEncryptionConfig {
    /// Builds a configuration only when every Mastodon-required secret is present.
    ///
    /// # Errors
    ///
    /// Returns the safe class of the missing secret.
    pub fn new(
        primary_key: SecretString,
        deterministic_key: SecretString,
        key_derivation_salt: SecretString,
    ) -> Result<Self, EncryptionConfigError> {
        if primary_key.is_empty() {
            return Err(EncryptionConfigError::MissingPrimaryKey);
        }
        if deterministic_key.is_empty() {
            return Err(EncryptionConfigError::MissingDeterministicKey);
        }
        if key_derivation_salt.is_empty() {
            return Err(EncryptionConfigError::MissingKeyDerivationSalt);
        }

        Ok(Self {
            primary_key,
            deterministic_key,
            key_derivation_salt,
        })
    }

    /// Decrypts a non-deterministic Rails encrypted string within a plaintext limit.
    ///
    /// The returned string and all intermediate plaintext and derived-key buffers are
    /// zeroized on drop.
    ///
    /// # Errors
    ///
    /// Returns only a safe failure class and never includes ciphertext, key, or plaintext.
    pub fn decrypt_string(
        &self,
        serialized: &str,
        max_plaintext_bytes: usize,
    ) -> Result<SecretString, ActiveRecordDecryptionError> {
        debug_assert!(!self.deterministic_key.is_empty());
        let envelope = parse_envelope(serialized)?;
        let plaintext = authenticate(
            &envelope,
            self.primary_key.expose_secret().as_bytes(),
            self.key_derivation_salt.expose_secret().as_bytes(),
            DerivationDigest::Sha256,
        )
        .or_else(|| {
            authenticate(
                &envelope,
                self.primary_key.expose_secret().as_bytes(),
                self.key_derivation_salt.expose_secret().as_bytes(),
                DerivationDigest::Sha1,
            )
        })
        .ok_or(ActiveRecordDecryptionError::AuthenticationFailed)?;

        if envelope.compressed {
            let inflated = inflate_bounded(&plaintext, max_plaintext_bytes)?;
            secret_from_utf8(&inflated)
        } else {
            if plaintext.len() > max_plaintext_bytes {
                return Err(ActiveRecordDecryptionError::PlaintextTooLarge);
            }
            secret_from_utf8(&plaintext)
        }
    }
}

impl fmt::Debug for ActiveRecordEncryptionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActiveRecordEncryptionConfig([REDACTED])")
    }
}

impl fmt::Display for ActiveRecordEncryptionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Safe Active Record encryption configuration failure classes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncryptionConfigError {
    /// The primary key was absent.
    MissingPrimaryKey,
    /// The deterministic key was absent.
    MissingDeterministicKey,
    /// The key derivation salt was absent.
    MissingKeyDerivationSalt,
}

impl fmt::Display for EncryptionConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingPrimaryKey => "Active Record encryption primary key is missing",
            Self::MissingDeterministicKey => {
                "Active Record encryption deterministic key is missing"
            }
            Self::MissingKeyDerivationSalt => {
                "Active Record encryption key derivation salt is missing"
            }
        })
    }
}

impl Error for EncryptionConfigError {}

/// Safe Rails encrypted-string decryption failure classes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActiveRecordDecryptionError {
    /// The compact JSON envelope has the wrong shape or values.
    InvalidEnvelope,
    /// A binary envelope field is not canonical padded standard Base64.
    InvalidBase64,
    /// The AES-GCM IV is not 12 bytes.
    InvalidIvLength,
    /// The AES-GCM authentication tag is not 16 bytes.
    InvalidTagLength,
    /// The envelope requests a non-UTF-8 plaintext encoding.
    UnsupportedEncoding,
    /// Neither the current nor legacy derived key authenticated the ciphertext.
    AuthenticationFailed,
    /// An authenticated payload marked as compressed is not valid zlib data.
    InvalidCompression,
    /// The authenticated plaintext exceeds the caller's limit.
    PlaintextTooLarge,
    /// The authenticated plaintext is not valid UTF-8.
    InvalidPlaintextEncoding,
}

impl fmt::Display for ActiveRecordDecryptionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEnvelope => "invalid encrypted-string envelope",
            Self::InvalidBase64 => "invalid encrypted-string Base64",
            Self::InvalidIvLength => "invalid encrypted-string IV length",
            Self::InvalidTagLength => "invalid encrypted-string authentication tag length",
            Self::UnsupportedEncoding => "unsupported encrypted-string encoding",
            Self::AuthenticationFailed => "encrypted-string authentication failed",
            Self::InvalidCompression => "invalid encrypted-string compression",
            Self::PlaintextTooLarge => "decrypted string exceeds configured limit",
            Self::InvalidPlaintextEncoding => "decrypted string is not UTF-8",
        })
    }
}

impl Error for ActiveRecordDecryptionError {}

struct Envelope {
    payload: Vec<u8>,
    iv: Vec<u8>,
    tag: Vec<u8>,
    compressed: bool,
}

fn parse_envelope(serialized: &str) -> Result<Envelope, ActiveRecordDecryptionError> {
    if serialized.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(ActiveRecordDecryptionError::InvalidEnvelope);
    }
    let value: Value = serde_json::from_str(serialized)
        .map_err(|_| ActiveRecordDecryptionError::InvalidEnvelope)?;
    let object = value
        .as_object()
        .filter(|object| object.len() == 2)
        .ok_or(ActiveRecordDecryptionError::InvalidEnvelope)?;
    require_one_key(serialized, "p")?;
    require_one_key(serialized, "h")?;
    let payload_text = object
        .get("p")
        .and_then(Value::as_str)
        .filter(|value| has_literal_json_string(serialized, value))
        .ok_or(ActiveRecordDecryptionError::InvalidEnvelope)?;
    let headers = object
        .get("h")
        .and_then(Value::as_object)
        .filter(|headers| (2..=4).contains(&headers.len()))
        .ok_or(ActiveRecordDecryptionError::InvalidEnvelope)?;

    for key in headers.keys() {
        if !matches!(key.as_str(), "iv" | "at" | "c" | "e") {
            return Err(ActiveRecordDecryptionError::InvalidEnvelope);
        }
        require_one_key(serialized, key)?;
    }
    let iv_text = string_header(headers, serialized, "iv")?;
    let tag_text = string_header(headers, serialized, "at")?;
    let compressed = match headers.get("c") {
        Some(Value::Bool(true)) => true,
        Some(_) => return Err(ActiveRecordDecryptionError::InvalidEnvelope),
        None => false,
    };
    if let Some(encoding) = headers.get("e") {
        let encoding = encoding
            .as_str()
            .filter(|value| has_literal_json_string(serialized, value))
            .ok_or(ActiveRecordDecryptionError::InvalidEnvelope)?;
        if decode_base64(encoding)?.as_slice() != b"UTF-8" {
            return Err(ActiveRecordDecryptionError::UnsupportedEncoding);
        }
    }

    let payload = decode_base64(payload_text)?;
    let iv = decode_base64(iv_text)?;
    let tag = decode_base64(tag_text)?;
    if iv.len() != GCM_IV_LENGTH {
        return Err(ActiveRecordDecryptionError::InvalidIvLength);
    }
    if tag.len() != GCM_TAG_LENGTH {
        return Err(ActiveRecordDecryptionError::InvalidTagLength);
    }

    Ok(Envelope {
        payload,
        iv,
        tag,
        compressed,
    })
}

fn require_one_key(serialized: &str, key: &str) -> Result<(), ActiveRecordDecryptionError> {
    let quoted = format!("\"{key}\"");
    if serialized.matches(&quoted).count() == 1 {
        Ok(())
    } else {
        Err(ActiveRecordDecryptionError::InvalidEnvelope)
    }
}

fn has_literal_json_string(serialized: &str, value: &str) -> bool {
    let quoted = format!("\"{value}\"");
    serialized.contains(&quoted)
}

fn string_header<'a>(
    headers: &'a serde_json::Map<String, Value>,
    serialized: &str,
    key: &str,
) -> Result<&'a str, ActiveRecordDecryptionError> {
    headers
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| has_literal_json_string(serialized, value))
        .ok_or(ActiveRecordDecryptionError::InvalidEnvelope)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, ActiveRecordDecryptionError> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| ActiveRecordDecryptionError::InvalidBase64)?;
    if STANDARD.encode(&decoded) == value {
        Ok(decoded)
    } else {
        Err(ActiveRecordDecryptionError::InvalidBase64)
    }
}

#[derive(Clone, Copy)]
enum DerivationDigest {
    Sha256,
    Sha1,
}

fn authenticate(
    envelope: &Envelope,
    password: &[u8],
    salt: &[u8],
    digest: DerivationDigest,
) -> Option<Zeroizing<Vec<u8>>> {
    let mut key = Zeroizing::new([0_u8; AES_KEY_LENGTH]);
    match digest {
        DerivationDigest::Sha256 => {
            pbkdf2_hmac::<Sha256>(password, salt, PBKDF2_ITERATIONS, &mut *key);
        }
        DerivationDigest::Sha1 => {
            pbkdf2_hmac::<Sha1>(password, salt, PBKDF2_ITERATIONS, &mut *key);
        }
    }

    let cipher = Aes256Gcm::new_from_slice(&key[..]).ok()?;
    let mut plaintext = Zeroizing::new(envelope.payload.clone());
    let plaintext_buffer: &mut Vec<u8> = plaintext.as_mut();
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&envelope.iv),
            b"",
            plaintext_buffer,
            Tag::from_slice(&envelope.tag),
        )
        .ok()?;
    Some(plaintext)
}

fn inflate_bounded(
    compressed: &[u8],
    max_plaintext_bytes: usize,
) -> Result<Zeroizing<Vec<u8>>, ActiveRecordDecryptionError> {
    let byte_limit = u64::try_from(max_plaintext_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut decoder = ZlibDecoder::new(compressed).take(byte_limit);
    let mut plaintext = Zeroizing::new(Vec::new());
    decoder
        .read_to_end(&mut plaintext)
        .map_err(|_| ActiveRecordDecryptionError::InvalidCompression)?;
    if plaintext.len() > max_plaintext_bytes {
        return Err(ActiveRecordDecryptionError::PlaintextTooLarge);
    }
    Ok(plaintext)
}

fn secret_from_utf8(bytes: &[u8]) -> Result<SecretString, ActiveRecordDecryptionError> {
    let plaintext = std::str::from_utf8(bytes)
        .map_err(|_| ActiveRecordDecryptionError::InvalidPlaintextEncoding)?;
    Ok(SecretString::new(plaintext.to_owned()))
}

/// Safe RSA signing-key preflight failure classes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsaKeyError {
    /// The private key is absent.
    MissingPrivateKey,
    /// The public key is absent.
    MissingPublicKey,
    /// The private key PEM or RSA parameters are invalid.
    CorruptPrivateKey,
    /// The public key PEM or RSA parameters are invalid.
    CorruptPublicKey,
    /// The private key does not derive the stored public modulus and exponent.
    KeyMismatch,
    /// Signing the fixed preflight challenge failed.
    SigningFailed,
    /// The stored public key did not verify the fixed preflight challenge.
    SignatureVerificationFailed,
}

impl fmt::Display for RsaKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingPrivateKey => "RSA private key is missing",
            Self::MissingPublicKey => "RSA public key is missing",
            Self::CorruptPrivateKey => "RSA private key is corrupt",
            Self::CorruptPublicKey => "RSA public key is corrupt",
            Self::KeyMismatch => "RSA private and public keys do not match",
            Self::SigningFailed => "RSA preflight challenge signing failed",
            Self::SignatureVerificationFailed => {
                "RSA preflight challenge signature verification failed"
            }
        })
    }
}

impl Error for RsaKeyError {}

/// Validates and exercises an RSA private key against its stored public key.
///
/// PKCS#1 and PKCS#8 private PEM and SPKI and PKCS#1 public PEM are accepted.
///
/// # Errors
///
/// Returns a targeted failure class without retaining or displaying key material.
pub fn validate_rsa_signing_keypair(
    private_key_pem: Option<&SecretString>,
    public_key_pem: &str,
) -> Result<(), RsaKeyError> {
    let private_key_pem = private_key_pem.ok_or(RsaKeyError::MissingPrivateKey)?;
    if private_key_pem.is_empty() {
        return Err(RsaKeyError::MissingPrivateKey);
    }
    if public_key_pem.is_empty() {
        return Err(RsaKeyError::MissingPublicKey);
    }

    let private_key = parse_private_key(private_key_pem.expose_secret())?;
    private_key
        .validate()
        .map_err(|_| RsaKeyError::CorruptPrivateKey)?;
    let public_key = parse_public_key(public_key_pem)?;
    let derived_public_key = RsaPublicKey::from(&private_key);
    if derived_public_key.n() != public_key.n() || derived_public_key.e() != public_key.e() {
        return Err(RsaKeyError::KeyMismatch);
    }

    let signature = SigningKey::<Sha256>::new(private_key)
        .try_sign(SIGNING_CHALLENGE)
        .map_err(|_| RsaKeyError::SigningFailed)?;
    VerifyingKey::<Sha256>::new(public_key)
        .verify(SIGNING_CHALLENGE, &signature)
        .map_err(|_| RsaKeyError::SignatureVerificationFailed)
}

fn parse_private_key(pem: &str) -> Result<RsaPrivateKey, RsaKeyError> {
    RsaPrivateKey::from_pkcs1_pem(pem)
        .or_else(|_| RsaPrivateKey::from_pkcs8_pem(pem))
        .map_err(|_| RsaKeyError::CorruptPrivateKey)
}

fn parse_public_key(pem: &str) -> Result<RsaPublicKey, RsaKeyError> {
    RsaPublicKey::from_public_key_pem(pem)
        .or_else(|_| RsaPublicKey::from_pkcs1_pem(pem))
        .map_err(|_| RsaKeyError::CorruptPublicKey)
}
