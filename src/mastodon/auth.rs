use std::fmt;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bcrypt::verify as verify_bcrypt;
use rsa::rand_core::{OsRng, RngCore};
use sha1::{Digest, Sha1};

const TOTP_STEP_SECONDS: i64 = 30;
const TOTP_DIGITS: u32 = 6;
const TOTP_ALLOWED_DRIFT: i64 = 1;
const TOTP_SECRET_LENGTH: usize = 32;
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthenticationFailure {
    InvalidPassword,
    InvalidTwoFactor,
    TwoFactorRequired,
}

impl fmt::Display for AuthenticationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPassword => "invalid password",
            Self::InvalidTwoFactor => "invalid two-factor code",
            Self::TwoFactorRequired => "two-factor authentication is required",
        })
    }
}

impl std::error::Error for AuthenticationFailure {}

#[cfg(feature = "test-support")]
static PASSWORD_VERIFICATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Test-only observation of entries to the bcrypt password-verification boundary.
#[cfg(feature = "test-support")]
#[must_use]
pub fn password_verification_count() -> usize {
    PASSWORD_VERIFICATIONS.load(std::sync::atomic::Ordering::SeqCst)
}

#[must_use]
pub fn verify_password(password: &str, encrypted_password: &str) -> bool {
    #[cfg(feature = "test-support")]
    PASSWORD_VERIFICATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    verify_bcrypt(password, encrypted_password).unwrap_or(false)
}

#[must_use]
pub fn random_auth_token(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[must_use]
pub fn random_totp_secret() -> String {
    let mut bytes = [0_u8; TOTP_SECRET_LENGTH];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .into_iter()
        .map(|byte| char::from(BASE32_ALPHABET[usize::from(byte & 31)]))
        .collect()
}

#[must_use]
pub fn random_backup_code() -> String {
    random_auth_token(8)
}

#[must_use]
pub fn valid_totp_secret(secret: &str) -> bool {
    !secret.is_empty() && decode_base32(secret).is_some_and(|decoded| !decoded.is_empty())
}

#[must_use]
pub fn verify_two_factor(
    secret: Option<&str>,
    backup_codes: &[String],
    attempt: &str,
    timestamp: i64,
    consumed_timestep: Option<i64>,
) -> TwoFactorVerification {
    if let Some(timestep) = verify_totp(secret, attempt, timestamp, consumed_timestep) {
        return TwoFactorVerification::Totp(timestep);
    }
    backup_codes
        .iter()
        .position(|stored| constant_time_backup_code_match(stored, attempt))
        .map_or(
            TwoFactorVerification::Invalid,
            TwoFactorVerification::BackupCode,
        )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TwoFactorVerification {
    Invalid,
    Totp(i64),
    BackupCode(usize),
}

fn verify_totp(
    secret: Option<&str>,
    attempt: &str,
    timestamp: i64,
    consumed_timestep: Option<i64>,
) -> Option<i64> {
    if attempt.len() != TOTP_DIGITS as usize || !attempt.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let secret = decode_base32(secret?)?;
    let timestep = timestamp.div_euclid(TOTP_STEP_SECONDS);
    for candidate in
        [0, -TOTP_ALLOWED_DRIFT, TOTP_ALLOWED_DRIFT].map(|offset| timestep.saturating_add(offset))
    {
        if consumed_timestep.is_some_and(|consumed| consumed >= candidate) {
            continue;
        }
        if hotp(&secret, candidate) == attempt {
            return Some(candidate);
        }
    }
    None
}

fn hotp(secret: &[u8], counter: i64) -> String {
    let mut key = [0_u8; 64];
    if secret.len() > key.len() {
        let mut digest = Sha1::new();
        digest.update(secret);
        key[..20].copy_from_slice(&digest.finalize());
    } else {
        key[..secret.len()].copy_from_slice(secret);
    }

    let mut message = [0_u8; 8];
    message.copy_from_slice(&counter.to_be_bytes());
    let mut inner = [0_u8; 64];
    let mut outer = [0_u8; 64];
    for index in 0..64 {
        inner[index] = key[index] ^ 0x36;
        outer[index] = key[index] ^ 0x5c;
    }
    let mut inner_digest = Sha1::new();
    inner_digest.update(inner);
    inner_digest.update(message);
    let inner_digest = inner_digest.finalize();
    let mut digest = Sha1::new();
    digest.update(outer);
    digest.update(inner_digest);
    let digest = digest.finalize();
    let offset = usize::from(digest[19] & 0x0f);
    let binary = (u32::from(digest[offset]) & 0x7f) << 24
        | u32::from(digest[offset + 1]) << 16
        | u32::from(digest[offset + 2]) << 8
        | u32::from(digest[offset + 3]);
    format!(
        "{:0width$}",
        binary % 10_u32.pow(TOTP_DIGITS),
        width = TOTP_DIGITS as usize
    )
}

fn decode_base32(secret: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(secret.len() * 5 / 8);
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in secret.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = match byte.to_ascii_uppercase() {
            b'A'..=b'Z' => byte.to_ascii_uppercase() - b'A',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | u32::from(value);
        bits = bits.saturating_add(5);
        while bits >= 8 {
            bits -= 8;
            output.push(u8::try_from(buffer >> bits).expect("base32 output byte is at most 255"));
            if bits == 0 {
                buffer = 0;
            } else {
                buffer &= (1_u32 << bits) - 1;
            }
        }
    }
    Some(output)
}

fn constant_time_backup_code_match(stored: &str, attempt: &str) -> bool {
    if stored.starts_with("$2") {
        return verify_bcrypt(attempt, stored).unwrap_or(false);
    }
    let stored = stored.as_bytes();
    let attempt = attempt.as_bytes();
    let mut difference = stored.len() ^ attempt.len();
    for index in 0..stored.len().max(attempt.len()) {
        difference |= usize::from(stored.get(index).copied().unwrap_or_default())
            ^ usize::from(attempt.get(index).copied().unwrap_or_default());
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use bcrypt::hash;

    use super::{
        TwoFactorVerification, random_backup_code, random_totp_secret, valid_totp_secret,
        verify_password, verify_two_factor,
    };

    #[test]
    fn verifies_rails_bcrypt_passwords_and_rejects_wrong_passwords() {
        let encrypted = hash("fixture-password", 4).expect("test bcrypt hash is generated");
        assert!(verify_password("fixture-password", &encrypted));
        assert!(!verify_password("wrong-password", &encrypted));
        assert!(verify_password(
            "fixture-password",
            "$2a$04$eYtbMaSJeYOgS7ENqTs6vezGAQVltj68iGTiHiIsdst5LyirUp5JC"
        ));
    }

    #[test]
    fn verifies_six_digit_rfc_totp_with_one_step_drift_and_replay_protection() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(
            verify_two_factor(Some(secret), &[], "287082", 59, None),
            TwoFactorVerification::Totp(1)
        );
        assert_eq!(
            verify_two_factor(Some(secret), &[], "287082", 59, Some(1)),
            TwoFactorVerification::Invalid
        );
        assert_eq!(
            verify_two_factor(Some(secret), &[], "081804", 1_111_111_109, None),
            TwoFactorVerification::Totp(37_037_036)
        );
    }

    #[test]
    fn consumes_plain_fixture_and_bcrypt_backup_codes_by_position() {
        let encrypted = hash("hashed-recovery", 4).expect("test bcrypt hash is generated");
        let codes = vec!["fixture-recovery-code".to_owned(), encrypted];
        assert_eq!(
            verify_two_factor(None, &codes, "fixture-recovery-code", 0, None),
            TwoFactorVerification::BackupCode(0)
        );
        assert_eq!(
            verify_two_factor(None, &codes, "hashed-recovery", 0, None),
            TwoFactorVerification::BackupCode(1)
        );
        assert_eq!(
            verify_two_factor(None, &codes, "unknown-code", 0, None),
            TwoFactorVerification::Invalid
        );
    }

    #[test]
    fn generates_compatible_totp_secrets_and_backup_codes() {
        let secret = random_totp_secret();
        assert_eq!(secret.len(), 32);
        assert!(
            secret
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte))
        );
        assert!(valid_totp_secret(&secret));

        let backup_code = random_backup_code();
        assert!(!backup_code.is_empty());
        assert!(backup_code.is_ascii());
    }
}
