use std::collections::BTreeSet;
use std::fmt;

use reqwest::header::{HeaderMap, HeaderName};
use serde_json::{Map, Value};

use super::normalization::{NormalizationError, NormalizationRule, normalize_pair};

pub(crate) const DEFAULT_MISMATCH_LIMIT: usize = 16;

#[derive(Clone, Debug)]
pub(crate) struct CapturedResponse {
    pub(crate) status: u16,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ObservedJson {
    Missing,
    Value(Value),
}

impl fmt::Display for ObservedJson {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("<missing>"),
            Self::Value(Value::Null) => formatter.write_str("null"),
            Self::Value(value) => write!(formatter, "{value}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Mismatch {
    Status {
        mastodon: u16,
        rust: u16,
    },
    Header {
        name: String,
        mastodon: Option<Vec<Vec<u8>>>,
        rust: Option<Vec<Vec<u8>>>,
    },
    Json {
        path: String,
        mastodon: ObservedJson,
        rust: ObservedJson,
    },
    InvalidJson {
        side: &'static str,
        message: String,
    },
}

impl fmt::Display for Mismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status { mastodon, rust } => {
                write!(formatter, "status: Mastodon={mastodon}, Rust={rust}")
            }
            Self::Header {
                name,
                mastodon,
                rust,
            } => write!(
                formatter,
                "header {name}: Mastodon={}, Rust={}",
                display_header(mastodon.as_deref()),
                display_header(rust.as_deref())
            ),
            Self::Json {
                path,
                mastodon,
                rust,
            } => write!(formatter, "JSON {path}: Mastodon={mastodon}, Rust={rust}"),
            Self::InvalidJson { side, message } => {
                write!(
                    formatter,
                    "JSON $: {side} response is invalid JSON: {message}"
                )
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MismatchReport {
    pub(crate) mismatches: Vec<Mismatch>,
    pub(crate) omitted: usize,
}

impl fmt::Display for MismatchReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "differential comparison found {} mismatch(es):",
            self.mismatches.len() + self.omitted
        )?;
        for mismatch in &self.mismatches {
            writeln!(formatter, "- {mismatch}")?;
        }
        if self.omitted > 0 {
            writeln!(
                formatter,
                "- ... {} additional mismatch(es) omitted",
                self.omitted
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for MismatchReport {}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ComparisonError {
    Normalization(NormalizationError),
    Mismatches(MismatchReport),
}

impl fmt::Display for ComparisonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normalization(error) => write!(formatter, "normalization failed: {error}"),
            Self::Mismatches(report) => report.fmt(formatter),
        }
    }
}

impl std::error::Error for ComparisonError {}

pub(crate) fn compare_responses(
    mastodon: &CapturedResponse,
    rust: &CapturedResponse,
    relevant_headers: &[HeaderName],
    normalizations: &[NormalizationRule],
    mismatch_limit: usize,
) -> Result<(), ComparisonError> {
    let (mut mastodon, mut rust) = (mastodon.clone(), rust.clone());
    normalize_pair(&mut mastodon, &mut rust, normalizations)
        .map_err(ComparisonError::Normalization)?;

    let mut collector = MismatchCollector::new(mismatch_limit);
    if mastodon.status != rust.status {
        collector.push(Mismatch::Status {
            mastodon: mastodon.status,
            rust: rust.status,
        });
    }

    let mut seen_headers = BTreeSet::new();
    for name in relevant_headers {
        if seen_headers.insert(name.as_str()) {
            let mastodon_values = header_values(&mastodon.headers, name);
            let rust_values = header_values(&rust.headers, name);
            if mastodon_values != rust_values {
                collector.push(Mismatch::Header {
                    name: name.as_str().to_owned(),
                    mastodon: mastodon_values,
                    rust: rust_values,
                });
            }
        }
    }

    let mastodon_json = serde_json::from_slice::<Value>(&mastodon.body);
    let rust_json = serde_json::from_slice::<Value>(&rust.body);
    match (mastodon_json, rust_json) {
        (Ok(mastodon), Ok(rust)) => {
            compare_json_at(&mastodon, &rust, "$", &mut collector);
        }
        (Err(error), Ok(_)) => collector.push(Mismatch::InvalidJson {
            side: "Mastodon",
            message: error.to_string(),
        }),
        (Ok(_), Err(error)) => collector.push(Mismatch::InvalidJson {
            side: "Rust",
            message: error.to_string(),
        }),
        (Err(mastodon), Err(rust)) => {
            collector.push(Mismatch::InvalidJson {
                side: "Mastodon",
                message: mastodon.to_string(),
            });
            collector.push(Mismatch::InvalidJson {
                side: "Rust",
                message: rust.to_string(),
            });
        }
    }

    collector.finish().map_err(ComparisonError::Mismatches)
}

pub(crate) fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonicalize_json(&object[key])))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        scalar => scalar.clone(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JsonDifference {
    pub(crate) path: String,
    pub(crate) mastodon: ObservedJson,
    pub(crate) rust: ObservedJson,
}

pub(crate) fn json_differences(
    mastodon: &Value,
    rust: &Value,
    mismatch_limit: usize,
) -> (Vec<JsonDifference>, usize) {
    let mut collector = MismatchCollector::new(mismatch_limit);
    compare_json_at(mastodon, rust, "$", &mut collector);
    let (mismatches, omitted) = collector.into_parts();
    (
        mismatches
            .into_iter()
            .filter_map(|mismatch| match mismatch {
                Mismatch::Json {
                    path,
                    mastodon,
                    rust,
                } => Some(JsonDifference {
                    path,
                    mastodon,
                    rust,
                }),
                _ => None,
            })
            .collect(),
        omitted,
    )
}

fn header_values(headers: &HeaderMap, name: &HeaderName) -> Option<Vec<Vec<u8>>> {
    let values = headers
        .get_all(name)
        .iter()
        .map(|value| value.as_bytes().to_vec())
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn display_header(values: Option<&[Vec<u8>]>) -> String {
    values.map_or_else(
        || "<missing>".to_owned(),
        |values| {
            values
                .iter()
                .map(|value| String::from_utf8_lossy(value).into_owned())
                .collect::<Vec<_>>()
                .join(", ")
        },
    )
}

fn compare_json_at(mastodon: &Value, rust: &Value, path: &str, collector: &mut MismatchCollector) {
    match (mastodon, rust) {
        (Value::Object(mastodon), Value::Object(rust)) => {
            let keys = mastodon.keys().chain(rust.keys()).collect::<BTreeSet<_>>();
            for key in keys {
                let child_path = json_child_path(path, key);
                match (mastodon.get(key), rust.get(key)) {
                    (Some(mastodon), Some(rust)) => {
                        compare_json_at(mastodon, rust, &child_path, collector);
                    }
                    (Some(mastodon), None) => collector.push(Mismatch::Json {
                        path: child_path,
                        mastodon: ObservedJson::Value(mastodon.clone()),
                        rust: ObservedJson::Missing,
                    }),
                    (None, Some(rust)) => collector.push(Mismatch::Json {
                        path: child_path,
                        mastodon: ObservedJson::Missing,
                        rust: ObservedJson::Value(rust.clone()),
                    }),
                    (None, None) => unreachable!("a union key must exist in at least one object"),
                }
            }
        }
        (Value::Array(mastodon), Value::Array(rust)) => {
            for index in 0..mastodon.len().max(rust.len()) {
                let child_path = format!("{path}[{index}]");
                match (mastodon.get(index), rust.get(index)) {
                    (Some(mastodon), Some(rust)) => {
                        compare_json_at(mastodon, rust, &child_path, collector);
                    }
                    (Some(mastodon), None) => collector.push(Mismatch::Json {
                        path: child_path,
                        mastodon: ObservedJson::Value(mastodon.clone()),
                        rust: ObservedJson::Missing,
                    }),
                    (None, Some(rust)) => collector.push(Mismatch::Json {
                        path: child_path,
                        mastodon: ObservedJson::Missing,
                        rust: ObservedJson::Value(rust.clone()),
                    }),
                    (None, None) => unreachable!("an in-range index must exist in one array"),
                }
            }
        }
        _ if mastodon == rust => {}
        _ => collector.push(Mismatch::Json {
            path: path.to_owned(),
            mastodon: ObservedJson::Value(mastodon.clone()),
            rust: ObservedJson::Value(rust.clone()),
        }),
    }
}

fn json_child_path(parent: &str, key: &str) -> String {
    if is_identifier(key) {
        format!("{parent}.{key}")
    } else {
        format!(
            "{parent}[{}]",
            serde_json::to_string(key).expect("a JSON object key is serializable")
        )
    }
}

fn is_identifier(key: &str) -> bool {
    let mut characters = key.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

struct MismatchCollector {
    limit: usize,
    mismatches: Vec<Mismatch>,
    omitted: usize,
}

impl MismatchCollector {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            mismatches: Vec::with_capacity(limit),
            omitted: 0,
        }
    }

    fn push(&mut self, mismatch: Mismatch) {
        if self.mismatches.len() < self.limit {
            self.mismatches.push(mismatch);
        } else {
            self.omitted += 1;
        }
    }

    fn finish(self) -> Result<(), MismatchReport> {
        if self.mismatches.is_empty() && self.omitted == 0 {
            Ok(())
        } else {
            Err(MismatchReport {
                mismatches: self.mismatches,
                omitted: self.omitted,
            })
        }
    }

    fn into_parts(self) -> (Vec<Mismatch>, usize) {
        (self.mismatches, self.omitted)
    }
}

#[cfg(test)]
mod tests {
    use reqwest::header::{CONTENT_TYPE, HeaderValue};
    use serde_json::json;

    use super::*;

    fn response(status: u16, content_type: Option<&str>, body: &str) -> CapturedResponse {
        let mut headers = HeaderMap::new();
        if let Some(content_type) = content_type {
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_str(content_type).expect("valid test header"),
            );
        }
        CapturedResponse {
            status,
            headers,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn equivalent_instance_response_ignores_object_order() {
        let mastodon = response(
            200,
            Some("application/json"),
            r#"{"domain":"fixture.invalid","usage":{"users":1}}"#,
        );
        let rust = response(
            200,
            Some("application/json"),
            r#"{"usage":{"users":1},"domain":"fixture.invalid"}"#,
        );

        assert!(
            compare_responses(
                &mastodon,
                &rust,
                &[CONTENT_TYPE],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .is_ok()
        );
    }

    #[test]
    fn intentional_status_header_and_json_mismatches_are_readable() {
        let mastodon = response(
            200,
            Some("application/json"),
            r#"{"configuration":{"limit":500}}"#,
        );
        let rust = response(
            503,
            Some("text/plain"),
            r#"{"configuration":{"limit":501}}"#,
        );

        let error = compare_responses(
            &mastodon,
            &rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .expect_err("intentional mismatches must return an error");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("status: Mastodon=200, Rust=503"));
        assert!(diagnostic.contains("header content-type"));
        assert!(diagnostic.contains("JSON $.configuration.limit"));
    }

    #[test]
    fn missing_json_is_distinct_from_null_and_reports_are_bounded() {
        let mastodon = response(200, None, r#"{"a":null,"b":1,"c":2}"#);
        let rust = response(200, None, r#"{"b":2,"c":3}"#);

        let ComparisonError::Mismatches(report) = compare_responses(&mastodon, &rust, &[], &[], 2)
            .expect_err("responses intentionally differ")
        else {
            panic!("expected structured mismatches");
        };
        assert_eq!(report.mismatches.len(), 2);
        assert_eq!(report.omitted, 1);
        assert_eq!(
            report.mismatches[0],
            Mismatch::Json {
                path: "$.a".to_owned(),
                mastodon: ObservedJson::Value(Value::Null),
                rust: ObservedJson::Missing,
            }
        );
    }

    #[test]
    fn arrays_and_arbitrary_precision_numbers_are_preserved() {
        let ordered = response(200, None, r#"{"ids":[123456789012345678901234567890,2]}"#);
        let reordered = response(200, None, r#"{"ids":[2,123456789012345678901234567890]}"#);
        let error = compare_responses(&ordered, &reordered, &[], &[], DEFAULT_MISMATCH_LIMIT)
            .expect_err("array order must remain observable");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("$.ids[0]"));
        assert!(diagnostic.contains("123456789012345678901234567890"));

        let canonical = canonicalize_json(&json!({
            "z": [123_456_789_012_345_678_901_234_567_890_u128, 2],
            "a": 1,
        }));
        assert_eq!(
            canonical["z"][0].to_string(),
            "123456789012345678901234567890"
        );
        assert_eq!(canonical["z"][1], 2);
    }
}
