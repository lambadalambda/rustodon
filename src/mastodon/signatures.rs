use std::error::Error;
use std::fmt;
use std::time::{Duration, SystemTime};

use axum::http::{HeaderMap, Method};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

use crate::crypto::{RsaKeyError, sign_rsa_sha256, verify_rsa_sha256};

const ALGORITHM: &str = "rsa-sha256";
const REQUEST_TARGET: &str = "(request-target)";
const CLOCK_SKEW_MARGIN: Duration = Duration::from_hours(1);
const EXPIRATION_WINDOW_LIMIT: Duration = Duration::from_hours(12);
const DEFAULT_EXPIRATION_WINDOW: Duration = Duration::from_mins(5);

/// The request fields covered by a draft Cavage HTTP signature.
pub struct HttpSignatureRequest<'a> {
    pub method: &'a Method,
    pub path_and_query: &'a str,
    pub headers: &'a HeaderMap,
    pub body: &'a [u8],
}

impl fmt::Debug for HttpSignatureRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpSignatureRequest")
            .field("method", self.method)
            .field("path_and_query", &self.path_and_query)
            .field("headers", self.headers)
            .field("body_length", &self.body.len())
            .finish()
    }
}

impl<'a> HttpSignatureRequest<'a> {
    #[must_use]
    pub const fn new(
        method: &'a Method,
        path_and_query: &'a str,
        headers: &'a HeaderMap,
        body: &'a [u8],
    ) -> Self {
        Self {
            method,
            path_and_query,
            headers,
            body,
        }
    }
}

/// A public key bound to the key ID resolved by the caller.
///
/// Resolving an `acct:` alias or fetching a remote key remains the caller's
/// responsibility. Verification rejects a signature whose key ID is not this
/// resolved key's ID, preventing a valid key from being reused for another
/// account or key record.
#[derive(Debug)]
pub struct HttpSignatureKey<'a> {
    pub key_id: &'a str,
    pub public_key_pem: &'a str,
}

/// The local account/keypair used to create a draft Cavage signature.
pub struct HttpSignatureSigner<'a> {
    pub key_id: &'a str,
    pub private_key_pem: &'a str,
}

impl fmt::Debug for HttpSignatureSigner<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpSignatureSigner")
            .field("key_id", &self.key_id)
            .field("private_key_pem", &"[REDACTED]")
            .finish()
    }
}

/// The identity and covered headers of a verified signature.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedHttpSignature {
    pub key_id: String,
    pub signed_headers: Vec<String>,
}

/// Safe failure classes for Mastodon's legacy HTTP signature profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpSignatureError {
    MissingSignatureHeader,
    UnsupportedMessageSignature,
    MalformedSignatureHeader,
    DuplicateSignatureParameter,
    MissingKeyId,
    MissingAlgorithm,
    MissingSignedHeaders,
    MissingSignature,
    UnsupportedAlgorithm,
    InvalidKeyId,
    KeyIdMismatch,
    MissingSignedHeader,
    InvalidHeaderValue,
    InvalidSignedHeader,
    MissingDate,
    InvalidDate,
    OutsideTimeWindow,
    MissingHost,
    MissingDigest,
    UnsupportedDigestAlgorithm,
    InvalidDigestEncoding,
    InvalidDigestLength,
    DigestMismatch,
    InvalidSignatureEncoding,
    InvalidPrivateKey,
    InvalidPublicKey,
    SignatureMismatch,
    UnsupportedMethod,
}

impl fmt::Display for HttpSignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingSignatureHeader => "signature header is missing",
            Self::UnsupportedMessageSignature => "HTTP Message Signatures are unsupported",
            Self::MalformedSignatureHeader => "signature header is malformed",
            Self::DuplicateSignatureParameter => "signature header contains duplicate parameters",
            Self::MissingKeyId => "signature keyId parameter is missing",
            Self::MissingAlgorithm => "signature algorithm parameter is missing",
            Self::MissingSignedHeaders => "signature headers parameter is missing",
            Self::MissingSignature => "signature parameter is missing",
            Self::UnsupportedAlgorithm => "only rsa-sha256 signatures are supported",
            Self::InvalidKeyId => "signature keyId is invalid",
            Self::KeyIdMismatch => "signature keyId does not match the resolved public key",
            Self::MissingSignedHeader => "a signed request header is missing",
            Self::InvalidHeaderValue => "a signed request header is invalid",
            Self::InvalidSignedHeader => "a signed header name is invalid",
            Self::MissingDate => "Mastodon requires the Date header to be signed",
            Self::InvalidDate => "the Date header is invalid",
            Self::OutsideTimeWindow => "signed request date is outside the acceptable time window",
            Self::MissingHost => "Mastodon requires the Host header to be signed for GET requests",
            Self::MissingDigest => {
                "Mastodon requires the Digest header to be signed for POST requests"
            }
            Self::UnsupportedDigestAlgorithm => {
                "Mastodon only supports SHA-256 in the Digest header"
            }
            Self::InvalidDigestEncoding => "the Digest value is not valid Base64",
            Self::InvalidDigestLength => "the Digest value is not a SHA-256 digest",
            Self::DigestMismatch => "the request body does not match the Digest header",
            Self::InvalidSignatureEncoding => "the signature value is not valid Base64",
            Self::InvalidPrivateKey => "the RSA private key is invalid",
            Self::InvalidPublicKey => "the RSA public key is invalid",
            Self::SignatureMismatch => "the HTTP signature does not match the request",
            Self::UnsupportedMethod => "only GET and POST requests can be signed",
        })
    }
}

impl Error for HttpSignatureError {}

/// Extracts the legacy signature key ID without verifying the request.
///
/// # Errors
///
/// Returns an error when the request uses an unsupported message-signature
/// header, has an invalid signature header, or omits `keyId`.
pub fn signature_key_id(headers: &HeaderMap) -> Result<Option<String>, HttpSignatureError> {
    if headers.contains_key("signature-input") {
        return Err(HttpSignatureError::UnsupportedMessageSignature);
    }
    let Some(raw_signature) = headers.get("signature") else {
        return Ok(None);
    };
    let raw_signature = raw_signature
        .to_str()
        .map_err(|_| HttpSignatureError::InvalidHeaderValue)?;
    let parameters = parse_signature_header(raw_signature)?;
    let key_id = parameters
        .get("keyId")
        .ok_or(HttpSignatureError::MissingKeyId)?;
    validate_key_id(key_id)?;
    Ok(Some(key_id.clone()))
}

/// Builds Mastodon's legacy `Digest` header for a request body.
#[must_use]
pub fn body_digest_header(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    format!("SHA-256={}", STANDARD.encode(digest))
}

/// Signs a GET or POST request using the standard Mastodon header order.
///
/// # Errors
///
/// Returns an error when the method, request headers, key ID, or RSA private
/// key cannot be used for a legacy signature.
pub fn sign_http_signature(
    request: &HttpSignatureRequest<'_>,
    signer: &HttpSignatureSigner<'_>,
) -> Result<String, HttpSignatureError> {
    let signed_headers = match *request.method {
        Method::GET => &["host", "date", REQUEST_TARGET][..],
        Method::POST => &["host", "date", "digest", REQUEST_TARGET][..],
        _ => return Err(HttpSignatureError::UnsupportedMethod),
    };

    sign_http_signature_with_headers(request, signer, signed_headers)
}

/// Signs a GET or POST request with an explicit Rails-compatible header order.
///
/// # Errors
///
/// Returns an error when the method, requested signed headers, key ID, or RSA
/// private key cannot be used for a legacy signature.
pub fn sign_http_signature_with_headers(
    request: &HttpSignatureRequest<'_>,
    signer: &HttpSignatureSigner<'_>,
    signed_headers: &[&str],
) -> Result<String, HttpSignatureError> {
    if !matches!(*request.method, Method::GET | Method::POST) {
        return Err(HttpSignatureError::UnsupportedMethod);
    }
    validate_key_id(signer.key_id)?;
    let signed_headers = normalize_signed_headers(signed_headers)?;
    if signed_headers.iter().any(|header| header == "digest") {
        verify_body_digest(request, &signed_headers)?;
    }
    let signed_string = build_signed_string(request, &signed_headers, true)?;
    let signature = sign_rsa_sha256(signer.private_key_pem, signed_string.as_bytes())
        .map_err(map_signing_error)?;

    Ok(format!(
        "keyId=\"{}\",algorithm=\"{}\",headers=\"{}\",signature=\"{}\"",
        signer.key_id,
        ALGORITHM,
        signed_headers.join(" "),
        STANDARD.encode(signature),
    ))
}

/// Verifies a Rails/Mastodon draft Cavage HTTP signature.
///
/// # Errors
///
/// Returns an error when the signature is malformed, unsupported, outside the
/// clock window, has an invalid body digest, does not match the resolved key,
/// or fails RSA verification.
pub fn verify_http_signature(
    request: &HttpSignatureRequest<'_>,
    key: &HttpSignatureKey<'_>,
    now: SystemTime,
) -> Result<VerifiedHttpSignature, HttpSignatureError> {
    if request.headers.contains_key("signature-input") {
        return Err(HttpSignatureError::UnsupportedMessageSignature);
    }

    let raw_signature = request
        .headers
        .get("signature")
        .ok_or(HttpSignatureError::MissingSignatureHeader)?
        .to_str()
        .map_err(|_| HttpSignatureError::InvalidHeaderValue)?;
    let parameters = parse_signature_header(raw_signature)?;
    let key_id = parameters
        .get("keyId")
        .ok_or(HttpSignatureError::MissingKeyId)?;
    if key_id != key.key_id {
        return Err(HttpSignatureError::KeyIdMismatch);
    }
    validate_key_id(key_id)?;
    if parameters.get("algorithm").map(String::as_str) != Some(ALGORITHM) {
        if parameters.contains_key("algorithm") {
            return Err(HttpSignatureError::UnsupportedAlgorithm);
        }
        return Err(HttpSignatureError::MissingAlgorithm);
    }
    let signed_headers = parameters
        .get("headers")
        .ok_or(HttpSignatureError::MissingSignedHeaders)
        .and_then(|headers| normalize_signed_headers_str(headers))?;
    let signature_value = parameters
        .get("signature")
        .ok_or(HttpSignatureError::MissingSignature)?;
    let signature = STANDARD
        .decode(signature_value)
        .map_err(|_| HttpSignatureError::InvalidSignatureEncoding)?;

    verify_signature_strength(request, &signed_headers)?;
    verify_time_window(
        request,
        &signed_headers,
        parameters.get("expires").map(String::as_str),
        now,
    )?;
    verify_body_digest(request, &signed_headers)?;

    let signed_string = build_signed_string(request, &signed_headers, true)?;
    let verified = verify_rsa_sha256(key.public_key_pem, signed_string.as_bytes(), &signature);
    let signature_matches = match verified {
        Ok(()) => true,
        Err(RsaKeyError::SignatureVerificationFailed) => false,
        Err(_) => return Err(HttpSignatureError::InvalidPublicKey),
    };
    if signature_matches {
        return Ok(VerifiedHttpSignature {
            key_id: key_id.clone(),
            signed_headers,
        });
    }

    if request.path_and_query.contains('?') {
        let signed_string = build_signed_string(request, &signed_headers, false)?;
        if verify_rsa_sha256(key.public_key_pem, signed_string.as_bytes(), &signature).is_ok() {
            return Ok(VerifiedHttpSignature {
                key_id: key_id.clone(),
                signed_headers,
            });
        }
    }

    Err(HttpSignatureError::SignatureMismatch)
}

fn parse_signature_header(
    raw: &str,
) -> Result<std::collections::BTreeMap<String, String>, HttpSignatureError> {
    let raw = raw.strip_prefix("Signature ").unwrap_or(raw);
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut parameters = std::collections::BTreeMap::new();

    loop {
        skip_spaces(bytes, &mut index);
        let key_start = index;
        while index < bytes.len() && is_token_byte(bytes[index]) {
            index += 1;
        }
        if key_start == index {
            return Err(HttpSignatureError::MalformedSignatureHeader);
        }
        let key = String::from_utf8(bytes[key_start..index].to_vec())
            .map_err(|_| HttpSignatureError::MalformedSignatureHeader)?;
        skip_spaces(bytes, &mut index);
        if bytes.get(index) != Some(&b'=') {
            return Err(HttpSignatureError::MalformedSignatureHeader);
        }
        index += 1;
        skip_spaces(bytes, &mut index);
        let value = if bytes.get(index) == Some(&b'"') {
            parse_quoted_value(bytes, &mut index)?
        } else {
            let value_start = index;
            while index < bytes.len() && is_token_byte(bytes[index]) {
                index += 1;
            }
            if value_start == index {
                return Err(HttpSignatureError::MalformedSignatureHeader);
            }
            String::from_utf8(bytes[value_start..index].to_vec())
                .map_err(|_| HttpSignatureError::MalformedSignatureHeader)?
        };
        if parameters.insert(key, value).is_some() {
            return Err(HttpSignatureError::DuplicateSignatureParameter);
        }
        skip_spaces(bytes, &mut index);
        if index == bytes.len() {
            return Ok(parameters);
        }
        if bytes[index] != b',' {
            return Err(HttpSignatureError::MalformedSignatureHeader);
        }
        index += 1;
    }
}

fn parse_quoted_value(bytes: &[u8], index: &mut usize) -> Result<String, HttpSignatureError> {
    *index += 1;
    let mut value = Vec::new();
    while *index < bytes.len() {
        match bytes[*index] {
            b'"' => {
                *index += 1;
                return String::from_utf8(value)
                    .map_err(|_| HttpSignatureError::MalformedSignatureHeader);
            }
            b'\\' => {
                *index += 1;
                let Some(byte) = bytes.get(*index) else {
                    return Err(HttpSignatureError::MalformedSignatureHeader);
                };
                value.push(b'\\');
                value.push(*byte);
                *index += 1;
            }
            byte if byte.is_ascii_control() => {
                return Err(HttpSignatureError::MalformedSignatureHeader);
            }
            byte => {
                value.push(byte);
                *index += 1;
            }
        }
    }
    Err(HttpSignatureError::MalformedSignatureHeader)
}

fn skip_spaces(bytes: &[u8], index: &mut usize) {
    while bytes.get(*index).is_some_and(u8::is_ascii_whitespace) {
        *index += 1;
    }
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn validate_key_id(key_id: &str) -> Result<(), HttpSignatureError> {
    if key_id.trim().is_empty()
        || key_id
            .bytes()
            .any(|byte| matches!(byte, b'"' | b'\\') || byte.is_ascii_control())
    {
        Err(HttpSignatureError::InvalidKeyId)
    } else {
        Ok(())
    }
}

fn normalize_signed_headers(headers: &[&str]) -> Result<Vec<String>, HttpSignatureError> {
    normalize_signed_headers_str(&headers.join(" "))
}

fn normalize_signed_headers_str(headers: &str) -> Result<Vec<String>, HttpSignatureError> {
    let headers = headers
        .split_ascii_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if headers.is_empty() {
        return Err(HttpSignatureError::MissingSignedHeaders);
    }
    if headers.iter().any(|header| {
        header != REQUEST_TARGET && (header.starts_with('(') || !header.bytes().all(is_token_byte))
    }) {
        return Err(HttpSignatureError::InvalidSignedHeader);
    }
    Ok(headers)
}

fn build_signed_string(
    request: &HttpSignatureRequest<'_>,
    signed_headers: &[String],
    include_query_string: bool,
) -> Result<String, HttpSignatureError> {
    signed_headers
        .iter()
        .map(|header| {
            let value = if header == REQUEST_TARGET {
                let path = if include_query_string {
                    request.path_and_query
                } else {
                    request
                        .path_and_query
                        .split_once('?')
                        .map_or(request.path_and_query, |(path, _)| path)
                };
                format!("{} {}", request.method.as_str().to_ascii_lowercase(), path)
            } else {
                request
                    .headers
                    .get(header)
                    .ok_or(HttpSignatureError::MissingSignedHeader)?
                    .to_str()
                    .map_err(|_| HttpSignatureError::InvalidHeaderValue)?
                    .to_owned()
            };
            Ok(format!("{header}: {value}"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|lines| lines.join("\n"))
}

fn verify_signature_strength(
    request: &HttpSignatureRequest<'_>,
    signed_headers: &[String],
) -> Result<(), HttpSignatureError> {
    if !signed_headers.iter().any(|header| header == "date") {
        return Err(HttpSignatureError::MissingDate);
    }
    if !signed_headers
        .iter()
        .any(|header| header == "digest" || header == REQUEST_TARGET)
    {
        return Err(HttpSignatureError::MissingDigest);
    }
    if request.method == Method::GET && !signed_headers.iter().any(|header| header == "host") {
        return Err(HttpSignatureError::MissingHost);
    }
    if request.method == Method::POST && !signed_headers.iter().any(|header| header == "digest") {
        return Err(HttpSignatureError::MissingDigest);
    }
    Ok(())
}

fn verify_time_window(
    request: &HttpSignatureRequest<'_>,
    signed_headers: &[String],
    expires_value: Option<&str>,
    now: SystemTime,
) -> Result<(), HttpSignatureError> {
    if !signed_headers.iter().any(|header| header == "date") {
        return Err(HttpSignatureError::MissingDate);
    }
    let date = request
        .headers
        .get("date")
        .ok_or(HttpSignatureError::MissingSignedHeader)?
        .to_str()
        .map_err(|_| HttpSignatureError::InvalidHeaderValue)?;
    let created = httpdate::parse_http_date(date).map_err(|_| HttpSignatureError::InvalidDate)?;
    if created > now + CLOCK_SKEW_MARGIN {
        return Err(HttpSignatureError::OutsideTimeWindow);
    }
    let expires = expires_value
        .map(parse_unix_timestamp)
        .transpose()?
        .unwrap_or(created + DEFAULT_EXPIRATION_WINDOW);
    let expires = std::cmp::min(expires, created + EXPIRATION_WINDOW_LIMIT);
    if now > expires + CLOCK_SKEW_MARGIN {
        return Err(HttpSignatureError::OutsideTimeWindow);
    }
    Ok(())
}

fn parse_unix_timestamp(value: &str) -> Result<SystemTime, HttpSignatureError> {
    let seconds = value
        .parse::<i64>()
        .map_err(|_| HttpSignatureError::InvalidDate)?;
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(seconds.unsigned_abs()))
            .ok_or(HttpSignatureError::InvalidDate)
    } else {
        SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(seconds.unsigned_abs()))
            .ok_or(HttpSignatureError::InvalidDate)
    }
}

fn verify_body_digest(
    request: &HttpSignatureRequest<'_>,
    signed_headers: &[String],
) -> Result<(), HttpSignatureError> {
    if !signed_headers.iter().any(|header| header == "digest") {
        return Ok(());
    }
    let digest_header = request
        .headers
        .get("digest")
        .ok_or(HttpSignatureError::MissingSignedHeader)?
        .to_str()
        .map_err(|_| HttpSignatureError::InvalidHeaderValue)?;
    let offered = digest_header
        .split(',')
        .filter_map(|digest| digest.split_once('='))
        .map(|(algorithm, value)| (algorithm.to_ascii_lowercase(), value.trim()))
        .collect::<Vec<_>>();
    let Some((_, received)) = offered.iter().find(|(algorithm, _)| algorithm == "sha-256") else {
        return Err(HttpSignatureError::UnsupportedDigestAlgorithm);
    };
    let expected = body_digest_header(request.body);
    if *received == &expected["SHA-256=".len()..] {
        return Ok(());
    }
    let decoded = STANDARD
        .decode(received)
        .map_err(|_| HttpSignatureError::InvalidDigestEncoding)?;
    if decoded.len() != Sha256::output_size() {
        return Err(HttpSignatureError::InvalidDigestLength);
    }
    Err(HttpSignatureError::DigestMismatch)
}

fn map_signing_error(error: RsaKeyError) -> HttpSignatureError {
    match error {
        RsaKeyError::CorruptPrivateKey
        | RsaKeyError::MissingPrivateKey
        | RsaKeyError::MissingPublicKey
        | RsaKeyError::KeyMismatch => HttpSignatureError::InvalidPrivateKey,
        RsaKeyError::SigningFailed | RsaKeyError::SignatureVerificationFailed => {
            HttpSignatureError::InvalidPrivateKey
        }
        RsaKeyError::CorruptPublicKey => HttpSignatureError::InvalidPublicKey,
    }
}
