use std::fmt;

use chrono::DateTime;
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{Map, Value};

use super::comparison::CapturedResponse;

const SENTINEL_KEY: &str = "$rustodon_differential";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NormalizationRule {
    RequestIdHeader {
        name: &'static str,
        reason: &'static str,
    },
    Rfc3339Timestamp {
        pointer: &'static str,
        reason: &'static str,
    },
    PrefixedRandomTestToken {
        pointer: &'static str,
        prefix: &'static str,
        reason: &'static str,
    },
}

pub(crate) const REQUEST_ID_HEADER: NormalizationRule = NormalizationRule::RequestIdHeader {
    name: "x-request-id",
    reason: "each server assigns an independent request correlation identifier",
};

pub(crate) const GENERATED_TIMESTAMP: NormalizationRule = NormalizationRule::Rfc3339Timestamp {
    pointer: "/generated_at",
    reason: "the response records the instant at which each implementation generated it",
};

pub(crate) const RANDOM_TEST_TOKEN: NormalizationRule =
    NormalizationRule::PrefixedRandomTestToken {
        pointer: "/token",
        prefix: "differential-",
        reason: "the compatibility case deliberately asks each implementation to mint a test token",
    };

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NormalizationError {
    pub(crate) rule: &'static str,
    pub(crate) target: &'static str,
    pub(crate) side: &'static str,
    pub(crate) detail: String,
}

impl fmt::Display for NormalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "rule {} rejected {} on {}: {}",
            self.rule, self.target, self.side, self.detail
        )
    }
}

impl std::error::Error for NormalizationError {}

pub(crate) fn normalize_pair(
    mastodon: &mut CapturedResponse,
    rust: &mut CapturedResponse,
    rules: &[NormalizationRule],
) -> Result<(), NormalizationError> {
    for rule in rules {
        match *rule {
            NormalizationRule::RequestIdHeader { name, .. } => {
                normalize_request_id_header(mastodon, rust, name)?;
            }
            NormalizationRule::Rfc3339Timestamp { pointer, .. } => {
                normalize_json_values(
                    mastodon,
                    rust,
                    "rfc3339-generated-timestamp",
                    pointer,
                    is_rfc3339,
                )?;
            }
            NormalizationRule::PrefixedRandomTestToken {
                pointer, prefix, ..
            } => {
                normalize_json_values(
                    mastodon,
                    rust,
                    "prefixed-random-test-token",
                    pointer,
                    |value| is_prefixed_token(value, prefix),
                )?;
            }
        }
    }
    Ok(())
}

impl NormalizationRule {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::RequestIdHeader { .. } => "request-id-header",
            Self::Rfc3339Timestamp { .. } => "rfc3339-generated-timestamp",
            Self::PrefixedRandomTestToken { .. } => "prefixed-random-test-token",
        }
    }

    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::RequestIdHeader { reason, .. }
            | Self::Rfc3339Timestamp { reason, .. }
            | Self::PrefixedRandomTestToken { reason, .. } => reason,
        }
    }
}

fn normalize_request_id_header(
    mastodon: &mut CapturedResponse,
    rust: &mut CapturedResponse,
    name: &'static str,
) -> Result<(), NormalizationError> {
    let header_name = HeaderName::from_static(name);
    let mastodon_value = one_header(&mastodon.headers, &header_name, "Mastodon")?;
    let rust_value = one_header(&rust.headers, &header_name, "Rust")?;

    validate_request_id(mastodon_value).map_err(|detail| NormalizationError {
        rule: "request-id-header",
        target: name,
        side: "Mastodon",
        detail,
    })?;
    validate_request_id(rust_value).map_err(|detail| NormalizationError {
        rule: "request-id-header",
        target: name,
        side: "Rust",
        detail,
    })?;

    let sentinel = HeaderValue::from_static("<normalized-request-id>");
    mastodon
        .headers
        .insert(header_name.clone(), sentinel.clone());
    rust.headers.insert(header_name, sentinel);
    Ok(())
}

fn one_header<'a>(
    headers: &'a reqwest::header::HeaderMap,
    name: &HeaderName,
    side: &'static str,
) -> Result<&'a HeaderValue, NormalizationError> {
    let values = headers.get_all(name).iter().collect::<Vec<_>>();
    match values.as_slice() {
        [value] => Ok(*value),
        [] => Err(NormalizationError {
            rule: "request-id-header",
            target: "x-request-id",
            side,
            detail: "target is missing".to_owned(),
        }),
        _ => Err(NormalizationError {
            rule: "request-id-header",
            target: "x-request-id",
            side,
            detail: "expected exactly one header value".to_owned(),
        }),
    }
}

fn validate_request_id(value: &HeaderValue) -> Result<(), String> {
    let text = value
        .to_str()
        .map_err(|_| "value is not visible ASCII".to_owned())?;
    if text.is_empty() || text.len() > 128 {
        return Err("value length must be between 1 and 128 bytes".to_owned());
    }
    if !text
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("value contains characters outside [A-Za-z0-9_-]".to_owned());
    }
    Ok(())
}

fn normalize_json_values(
    mastodon: &mut CapturedResponse,
    rust: &mut CapturedResponse,
    rule: &'static str,
    pointer: &'static str,
    validate: impl Fn(&Value) -> bool,
) -> Result<(), NormalizationError> {
    let mut mastodon_json = parse_json(&mastodon.body, rule, pointer, "Mastodon")?;
    let mut rust_json = parse_json(&rust.body, rule, pointer, "Rust")?;

    validate_pointer(&mastodon_json, rule, pointer, "Mastodon", &validate)?;
    validate_pointer(&rust_json, rule, pointer, "Rust", &validate)?;

    let sentinel = typed_sentinel(rule);
    *mastodon_json
        .pointer_mut(pointer)
        .expect("the pointer was validated before replacement") = sentinel.clone();
    *rust_json
        .pointer_mut(pointer)
        .expect("the pointer was validated before replacement") = sentinel;
    mastodon.body = serde_json::to_vec(&mastodon_json).expect("a JSON value is serializable");
    rust.body = serde_json::to_vec(&rust_json).expect("a JSON value is serializable");
    Ok(())
}

fn parse_json(
    body: &[u8],
    rule: &'static str,
    pointer: &'static str,
    side: &'static str,
) -> Result<Value, NormalizationError> {
    serde_json::from_slice(body).map_err(|error| NormalizationError {
        rule,
        target: pointer,
        side,
        detail: format!("response is not JSON: {error}"),
    })
}

fn validate_pointer(
    document: &Value,
    rule: &'static str,
    pointer: &'static str,
    side: &'static str,
    validate: &impl Fn(&Value) -> bool,
) -> Result<(), NormalizationError> {
    let Some(value) = document.pointer(pointer) else {
        return Err(NormalizationError {
            rule,
            target: pointer,
            side,
            detail: "target is missing".to_owned(),
        });
    };
    if !validate(value) {
        return Err(NormalizationError {
            rule,
            target: pointer,
            side,
            detail: format!("target has malformed shape: {value}"),
        });
    }
    Ok(())
}

fn is_rfc3339(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| DateTime::parse_from_rfc3339(text).is_ok())
}

fn is_prefixed_token(value: &Value, prefix: &str) -> bool {
    value.as_str().is_some_and(|text| {
        text.strip_prefix(prefix).is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    })
}

fn typed_sentinel(kind: &str) -> Value {
    Value::Object(Map::from_iter([(
        SENTINEL_KEY.to_owned(),
        Value::String(kind.to_owned()),
    )]))
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderMap, HeaderValue};

    use super::super::comparison::{ComparisonError, DEFAULT_MISMATCH_LIMIT, compare_responses};
    use super::*;

    fn response(body: &str, request_id: Option<&str>) -> CapturedResponse {
        let mut headers = HeaderMap::new();
        if let Some(request_id) = request_id {
            headers.insert(
                "x-request-id",
                HeaderValue::from_str(request_id).expect("valid test header"),
            );
        }
        CapturedResponse {
            status: 200,
            headers,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn named_rules_have_narrow_documented_reasons() {
        for rule in [REQUEST_ID_HEADER, GENERATED_TIMESTAMP, RANDOM_TEST_TOKEN] {
            assert!(!rule.name().is_empty());
            assert!(rule.reason().len() > 20);
        }
    }

    #[test]
    fn valid_nondeterminism_is_replaced_after_both_sides_validate() {
        let mastodon = response(
            r#"{"generated_at":"2026-07-01T12:00:00Z","token":"differential-left","sibling":1}"#,
            Some("request-left"),
        );
        let rust = response(
            r#"{"sibling":1,"token":"differential-right","generated_at":"2026-07-01T12:00:01+00:00"}"#,
            Some("request-right"),
        );

        assert!(
            compare_responses(
                &mastodon,
                &rust,
                &[HeaderName::from_static("x-request-id")],
                &[REQUEST_ID_HEADER, GENERATED_TIMESTAMP, RANDOM_TEST_TOKEN],
                DEFAULT_MISMATCH_LIMIT,
            )
            .is_ok()
        );
    }

    #[test]
    fn normalization_does_not_hide_sibling_differences() {
        let mastodon = response(
            r#"{"generated_at":"2026-07-01T12:00:00Z","sibling":1}"#,
            None,
        );
        let rust = response(
            r#"{"generated_at":"2026-07-01T12:00:01Z","sibling":2}"#,
            None,
        );

        let error = compare_responses(
            &mastodon,
            &rust,
            &[],
            &[GENERATED_TIMESTAMP],
            DEFAULT_MISMATCH_LIMIT,
        )
        .expect_err("the sibling difference must remain visible");
        assert!(error.to_string().contains("JSON $.sibling"));
    }

    #[test]
    fn missing_or_malformed_targets_are_rejected_instead_of_hidden() {
        let mastodon = response(r#"{"generated_at":null}"#, None);
        let rust = response(r#"{"other":"2026-07-01T12:00:01Z"}"#, None);

        let ComparisonError::Normalization(error) = compare_responses(
            &mastodon,
            &rust,
            &[],
            &[GENERATED_TIMESTAMP],
            DEFAULT_MISMATCH_LIMIT,
        )
        .expect_err("malformed normalization targets must fail") else {
            panic!("expected a normalization failure");
        };
        assert_eq!(error.side, "Mastodon");
        assert!(error.detail.contains("malformed shape"));

        let mastodon = response("{}", Some("valid-id"));
        let rust = response("{}", None);
        let error = compare_responses(
            &mastodon,
            &rust,
            &[],
            &[REQUEST_ID_HEADER],
            DEFAULT_MISMATCH_LIMIT,
        )
        .expect_err("a missing header target must fail");
        assert!(error.to_string().contains("target is missing"));
    }
}
