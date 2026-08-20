use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::{Map, Value};
use sqlx::{Connection, PgConnection};

use super::comparison::{
    DEFAULT_MISMATCH_LIMIT, ObservedJson, canonicalize_json, json_differences,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum TableSelection<'a> {
    AllPublic,
    Only(&'a [&'a str]),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TableSnapshot {
    pub(crate) name: String,
    pub(crate) primary_key: Vec<String>,
    pub(crate) rows: Vec<Value>,
}

impl TableSnapshot {
    pub(crate) fn new(
        name: impl Into<String>,
        primary_key: Vec<String>,
        rows: Vec<Value>,
    ) -> Result<Self, DatabaseError> {
        let name = name.into();
        let rows = rows
            .into_iter()
            .map(|row| canonicalize_json(&row))
            .collect::<Vec<Value>>();
        for row in &rows {
            let object = row.as_object().ok_or_else(|| {
                DatabaseError::InvalidSnapshot(format!(
                    "table {name} contains a row that is not a JSON object"
                ))
            })?;
            for column in &primary_key {
                if !object.contains_key(column) {
                    return Err(DatabaseError::InvalidSnapshot(format!(
                        "table {name} row is missing primary-key column {column}"
                    )));
                }
            }
        }
        Ok(Self {
            name,
            primary_key,
            rows,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DatabaseSnapshot {
    pub(crate) tables: BTreeMap<String, TableSnapshot>,
}

impl DatabaseSnapshot {
    pub(crate) fn new(tables: Vec<TableSnapshot>) -> Result<Self, DatabaseError> {
        let mut by_name = BTreeMap::new();
        for table in tables {
            let name = table.name.clone();
            if by_name.insert(name.clone(), table).is_some() {
                return Err(DatabaseError::InvalidSnapshot(format!(
                    "duplicate table snapshot {name}"
                )));
            }
        }
        Ok(Self { tables: by_name })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DatabaseMismatchKind {
    TableAdded,
    TableDeleted,
    PrimaryKeyChanged,
    RowAdded,
    RowDeleted,
    RowChanged,
    MultisetCountChanged,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DatabaseMismatch {
    pub(crate) table: String,
    pub(crate) key: Option<String>,
    pub(crate) path: String,
    pub(crate) kind: DatabaseMismatchKind,
    pub(crate) mastodon: ObservedJson,
    pub(crate) rust: ObservedJson,
}

impl DatabaseMismatch {
    fn redact_secrets(&mut self) {
        self.mastodon = redact_observation(&self.table, &self.path, &self.mastodon);
        self.rust = redact_observation(&self.table, &self.path, &self.rust);
    }
}

impl fmt::Display for DatabaseMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_with_labels(formatter, "Mastodon", "Rust")
    }
}

impl DatabaseMismatch {
    fn fmt_with_labels(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        mastodon_label: &str,
        rust_label: &str,
    ) -> fmt::Result {
        let key = self
            .key
            .as_deref()
            .map_or_else(|| "<no key>".to_owned(), ToOwned::to_owned);
        write!(
            formatter,
            "table {} key {} at {} ({:?}): {}={}, {}={}",
            self.table,
            key,
            self.path,
            self.kind,
            mastodon_label,
            self.mastodon,
            rust_label,
            self.rust
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DatabaseMismatchReport {
    pub(crate) mismatches: Vec<DatabaseMismatch>,
    pub(crate) omitted: usize,
    mastodon_label: String,
    rust_label: String,
}

impl fmt::Display for DatabaseMismatchReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "database comparison found {} mismatch(es):",
            self.mismatches.len() + self.omitted
        )?;
        for mismatch in &self.mismatches {
            formatter.write_str("- ")?;
            mismatch.fmt_with_labels(formatter, &self.mastodon_label, &self.rust_label)?;
            formatter.write_str("\n")?;
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

impl std::error::Error for DatabaseMismatchReport {}

#[derive(Debug)]
pub(crate) enum DatabaseError {
    Sqlx(sqlx::Error),
    InvalidSelection(String),
    InvalidSnapshot(String),
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlx(error) => write!(formatter, "database snapshot query failed: {error}"),
            Self::InvalidSelection(message) => {
                write!(formatter, "invalid database table selection: {message}")
            }
            Self::InvalidSnapshot(message) => {
                write!(formatter, "invalid database snapshot: {message}")
            }
        }
    }
}

impl std::error::Error for DatabaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlx(error) => Some(error),
            Self::InvalidSelection(_) | Self::InvalidSnapshot(_) => None,
        }
    }
}

impl From<sqlx::Error> for DatabaseError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

pub(crate) async fn snapshot_database(
    database_url: &str,
    selection: TableSelection<'_>,
) -> Result<DatabaseSnapshot, DatabaseError> {
    let mut connection = PgConnection::connect(database_url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let available = sqlx::query_scalar::<_, String>(
        "SELECT c.relname::text \
         FROM pg_catalog.pg_class AS c \
         JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' \
           AND c.relkind IN ('r', 'p', 'm') \
           AND NOT c.relispartition \
         ORDER BY c.relname",
    )
    .fetch_all(&mut *transaction)
    .await?;
    let selected = select_tables(&available, selection)?;
    let mut tables = Vec::with_capacity(selected.len());
    for name in selected {
        let primary_key = sqlx::query_scalar::<_, String>(
            "SELECT a.attname::text \
             FROM pg_catalog.pg_index AS i \
             JOIN pg_catalog.pg_class AS c ON c.oid = i.indrelid \
             JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace \
             CROSS JOIN LATERAL unnest(i.indkey) WITH ORDINALITY AS k(attnum, position) \
             JOIN pg_catalog.pg_attribute AS a \
               ON a.attrelid = i.indrelid AND a.attnum = k.attnum \
             WHERE n.nspname = 'public' AND c.relname = $1 AND i.indisprimary \
             ORDER BY k.position",
        )
        .bind(&name)
        .fetch_all(&mut *transaction)
        .await?;

        // The identifier comes only from pg_catalog (or a selection validated
        // against it), and is quoted again before entering this SELECT-only SQL.
        let quoted = name.replace('"', "\"\"");
        let query = format!("SELECT to_jsonb(t) FROM public.\"{quoted}\" AS t");
        let rows = sqlx::query_scalar::<_, Value>(&query)
            .fetch_all(&mut *transaction)
            .await?;
        tables.push(TableSnapshot::new(name, primary_key, rows)?);
    }
    transaction.commit().await?;
    DatabaseSnapshot::new(tables)
}

pub(crate) fn compare_database_snapshots(
    mastodon: &DatabaseSnapshot,
    rust: &DatabaseSnapshot,
    limit: usize,
) -> Result<(), DatabaseMismatchReport> {
    compare_database_snapshots_with_labels(mastodon, rust, "Mastodon", "Rust", limit)
}

pub(crate) fn compare_database_snapshots_with_labels(
    mastodon: &DatabaseSnapshot,
    rust: &DatabaseSnapshot,
    mastodon_label: &str,
    rust_label: &str,
    limit: usize,
) -> Result<(), DatabaseMismatchReport> {
    let mut collector =
        DatabaseMismatchCollector::new(limit, mastodon_label.to_owned(), rust_label.to_owned());
    let names = mastodon
        .tables
        .keys()
        .chain(rust.tables.keys())
        .collect::<BTreeSet<_>>();
    for name in names {
        match (mastodon.tables.get(name), rust.tables.get(name)) {
            (Some(mastodon), Some(rust)) => compare_table(mastodon, rust, &mut collector),
            (Some(_), None) => collector.push(DatabaseMismatch {
                table: name.clone(),
                key: None,
                path: "$".to_owned(),
                kind: DatabaseMismatchKind::TableDeleted,
                mastodon: ObservedJson::Value(Value::String("table".to_owned())),
                rust: ObservedJson::Missing,
            }),
            (None, Some(_)) => collector.push(DatabaseMismatch {
                table: name.clone(),
                key: None,
                path: "$".to_owned(),
                kind: DatabaseMismatchKind::TableAdded,
                mastodon: ObservedJson::Missing,
                rust: ObservedJson::Value(Value::String("table".to_owned())),
            }),
            (None, None) => unreachable!("a union table name must exist in one snapshot"),
        }
    }
    collector.finish()
}

fn select_tables(
    available: &[String],
    selection: TableSelection<'_>,
) -> Result<Vec<String>, DatabaseError> {
    match selection {
        TableSelection::AllPublic => Ok(available.to_vec()),
        TableSelection::Only(requested) => {
            let available = available
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let mut selected = BTreeSet::new();
            for name in requested {
                if !available.contains(name) {
                    return Err(DatabaseError::InvalidSelection(format!(
                        "table {name:?} is not a persisted table or materialized view in public"
                    )));
                }
                if !selected.insert(*name) {
                    return Err(DatabaseError::InvalidSelection(format!(
                        "table {name:?} was selected more than once"
                    )));
                }
            }
            Ok(selected.into_iter().map(str::to_owned).collect())
        }
    }
}

fn compare_table(
    mastodon: &TableSnapshot,
    rust: &TableSnapshot,
    collector: &mut DatabaseMismatchCollector,
) {
    if mastodon.primary_key != rust.primary_key {
        collector.push(DatabaseMismatch {
            table: mastodon.name.clone(),
            key: None,
            path: "$primary_key".to_owned(),
            kind: DatabaseMismatchKind::PrimaryKeyChanged,
            mastodon: ObservedJson::Value(Value::Array(
                mastodon
                    .primary_key
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            )),
            rust: ObservedJson::Value(Value::Array(
                rust.primary_key
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            )),
        });
        return;
    }
    if mastodon.primary_key.is_empty() {
        compare_multiset_table(mastodon, rust, collector);
    } else {
        compare_keyed_table(mastodon, rust, collector);
    }
}

fn compare_keyed_table(
    mastodon_table: &TableSnapshot,
    rust_table: &TableSnapshot,
    collector: &mut DatabaseMismatchCollector,
) {
    let table_name = mastodon_table.name.clone();
    let mastodon_rows = keyed_rows(mastodon_table);
    let rust_rows = keyed_rows(rust_table);
    let keys = mastodon_rows
        .keys()
        .chain(rust_rows.keys())
        .collect::<BTreeSet<_>>();
    for key in keys {
        match (mastodon_rows.get(key), rust_rows.get(key)) {
            (Some(mastodon), Some(rust)) => {
                let remaining = collector.limit.saturating_sub(collector.mismatches.len());
                let (differences, omitted) = json_differences(mastodon, rust, remaining);
                for difference in differences {
                    collector.push(DatabaseMismatch {
                        table: table_name.clone(),
                        key: Some(key.clone()),
                        path: difference.path,
                        kind: DatabaseMismatchKind::RowChanged,
                        mastodon: difference.mastodon,
                        rust: difference.rust,
                    });
                }
                collector.omitted += omitted;
            }
            (Some(mastodon), None) => collector.push(DatabaseMismatch {
                table: table_name.clone(),
                key: Some(key.clone()),
                path: "$".to_owned(),
                kind: DatabaseMismatchKind::RowDeleted,
                mastodon: ObservedJson::Value((*mastodon).clone()),
                rust: ObservedJson::Missing,
            }),
            (None, Some(rust)) => collector.push(DatabaseMismatch {
                table: table_name.clone(),
                key: Some(key.clone()),
                path: "$".to_owned(),
                kind: DatabaseMismatchKind::RowAdded,
                mastodon: ObservedJson::Missing,
                rust: ObservedJson::Value((*rust).clone()),
            }),
            (None, None) => unreachable!("a union primary key must exist in one snapshot"),
        }
    }
}

fn keyed_rows(table: &TableSnapshot) -> BTreeMap<String, &Value> {
    table
        .rows
        .iter()
        .map(|row| (primary_key(row, &table.primary_key), row))
        .collect()
}

fn primary_key(row: &Value, columns: &[String]) -> String {
    let object = row
        .as_object()
        .expect("table snapshot construction validates object rows");
    let key = Value::Object(
        columns
            .iter()
            .map(|column| {
                (
                    column.clone(),
                    object
                        .get(column)
                        .expect("table snapshot construction validates primary keys")
                        .clone(),
                )
            })
            .collect::<Map<_, _>>(),
    );
    key.to_string()
}

fn compare_multiset_table(
    mastodon: &TableSnapshot,
    rust: &TableSnapshot,
    collector: &mut DatabaseMismatchCollector,
) {
    let mastodon_rows = multiset(&mastodon.rows);
    let rust_rows = multiset(&rust.rows);
    let rows = mastodon_rows
        .keys()
        .chain(rust_rows.keys())
        .collect::<BTreeSet<_>>();
    for row in rows {
        let mastodon_count = mastodon_rows.get(row).copied().unwrap_or_default();
        let rust_count = rust_rows.get(row).copied().unwrap_or_default();
        if mastodon_count != rust_count {
            collector.push(DatabaseMismatch {
                table: mastodon.name.clone(),
                key: Some(format!("multiset row {row}")),
                path: "$count".to_owned(),
                kind: DatabaseMismatchKind::MultisetCountChanged,
                mastodon: ObservedJson::Value(Value::from(mastodon_count)),
                rust: ObservedJson::Value(Value::from(rust_count)),
            });
        }
    }
}

fn multiset(rows: &[Value]) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(row.to_string()).or_default() += 1;
    }
    counts
}

struct DatabaseMismatchCollector {
    limit: usize,
    mismatches: Vec<DatabaseMismatch>,
    omitted: usize,
    mastodon_label: String,
    rust_label: String,
}

impl DatabaseMismatchCollector {
    fn new(limit: usize, mastodon_label: String, rust_label: String) -> Self {
        Self {
            limit,
            mismatches: Vec::with_capacity(limit),
            omitted: 0,
            mastodon_label,
            rust_label,
        }
    }

    fn push(&mut self, mismatch: DatabaseMismatch) {
        let mut mismatch = mismatch;
        mismatch.redact_secrets();
        if self.mismatches.len() < self.limit {
            self.mismatches.push(mismatch);
        } else {
            self.omitted += 1;
        }
    }

    fn finish(self) -> Result<(), DatabaseMismatchReport> {
        if self.mismatches.is_empty() && self.omitted == 0 {
            Ok(())
        } else {
            Err(DatabaseMismatchReport {
                mismatches: self.mismatches,
                omitted: self.omitted,
                mastodon_label: self.mastodon_label,
                rust_label: self.rust_label,
            })
        }
    }
}

fn redact_observation(table: &str, path: &str, observation: &ObservedJson) -> ObservedJson {
    let secret_columns: &[&str] = match table {
        "accounts" | "keypairs" => &["private_key"],
        "email_subscriptions" => &["confirmation_token"],
        "fasp_providers" => &["server_private_key_pem"],
        "generated_annual_reports" => &["share_key"],
        "oauth_access_grants" => &["token"],
        "oauth_access_tokens" => &["refresh_token", "token"],
        "oauth_applications" | "webhooks" => &["secret"],
        "session_activations" => &["session_id"],
        "users" => &[
            "confirmation_token",
            "encrypted_password",
            "otp_backup_codes",
            "otp_secret",
            "reset_password_token",
            "sign_in_token",
        ],
        "web_push_subscriptions" => &["data", "endpoint", "key_auth", "key_p256dh"],
        _ => &[],
    };
    if secret_columns
        .iter()
        .any(|column| path == format!("$.{column}") || path.starts_with(&format!("$.{column}[")))
    {
        return match observation {
            ObservedJson::Missing => ObservedJson::Missing,
            ObservedJson::Value(_) => ObservedJson::Value(Value::String("[REDACTED]".to_owned())),
        };
    }

    match observation {
        ObservedJson::Missing => ObservedJson::Missing,
        ObservedJson::Value(Value::Object(object)) => {
            let mut object = object.clone();
            for column in secret_columns {
                if object.contains_key(*column) {
                    object.insert((*column).to_owned(), Value::String("[REDACTED]".to_owned()));
                }
            }
            ObservedJson::Value(Value::Object(object))
        }
        ObservedJson::Value(value) => ObservedJson::Value(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn table(name: &str, primary_key: &[&str], rows: Vec<Value>) -> TableSnapshot {
        TableSnapshot::new(
            name,
            primary_key
                .iter()
                .map(|column| (*column).to_owned())
                .collect(),
            rows,
        )
        .expect("test table should be valid")
    }

    fn database(table: TableSnapshot) -> DatabaseSnapshot {
        DatabaseSnapshot::new(vec![table]).expect("test database should be valid")
    }

    #[test]
    fn primary_key_comparison_reports_changed_added_and_deleted_rows() {
        let mastodon = database(table(
            "accounts",
            &["id"],
            vec![
                json!({"id": 1, "name": "old"}),
                json!({"id": 2, "name": "deleted"}),
            ],
        ));
        let rust = database(table(
            "accounts",
            &["id"],
            vec![
                json!({"name": "new", "id": 1}),
                json!({"id": 3, "name": "added"}),
            ],
        ));

        let report = compare_database_snapshots(&mastodon, &rust, DEFAULT_MISMATCH_LIMIT)
            .expect_err("database rows intentionally differ");
        assert_eq!(report.mismatches.len(), 3);
        let diagnostic = report.to_string();
        assert!(diagnostic.contains("table accounts"));
        assert!(diagnostic.contains(r#"{"id":1}"#));
        assert!(diagnostic.contains("$.name"));
        assert!(
            report
                .mismatches
                .iter()
                .any(|mismatch| mismatch.kind == DatabaseMismatchKind::RowAdded)
        );
        assert!(
            report
                .mismatches
                .iter()
                .any(|mismatch| mismatch.kind == DatabaseMismatchKind::RowDeleted)
        );
    }

    #[test]
    fn no_primary_key_tables_use_a_multiset_not_row_order() {
        let first = json!({"event": "one", "number": 123_456_789_012_345_678_901_234_567_890_u128});
        let second = json!({"event": "two"});
        let mastodon = database(table(
            "events",
            &[],
            vec![first.clone(), second.clone(), first.clone()],
        ));
        let reordered = database(table(
            "events",
            &[],
            vec![first.clone(), first.clone(), second],
        ));
        assert!(compare_database_snapshots(&mastodon, &reordered, DEFAULT_MISMATCH_LIMIT).is_ok());

        let changed = database(table("events", &[], vec![first]));
        let report = compare_database_snapshots(&mastodon, &changed, DEFAULT_MISMATCH_LIMIT)
            .expect_err("multiset count intentionally differs");
        assert_eq!(
            report.mismatches[0].kind,
            DatabaseMismatchKind::MultisetCountChanged
        );
        assert!(report.to_string().contains("$count"));
    }

    #[test]
    fn selected_table_names_must_exist_and_must_not_repeat() {
        let available = vec!["accounts".to_owned(), "statuses".to_owned()];
        assert_eq!(
            select_tables(&available, TableSelection::Only(&["statuses"]))
                .expect("known selection should pass"),
            vec!["statuses"]
        );
        assert!(
            select_tables(
                &available,
                TableSelection::Only(&["accounts; DROP TABLE x"])
            )
            .is_err()
        );
        assert!(
            select_tables(&available, TableSelection::Only(&["accounts", "accounts"])).is_err()
        );
    }

    #[test]
    fn self_comparison_diagnostics_name_the_side_and_phase() {
        let before = database(table(
            "accounts",
            &["id"],
            vec![json!({"id": 1, "name": "before"})],
        ));
        let after = database(table(
            "accounts",
            &["id"],
            vec![json!({"id": 1, "name": "after"})],
        ));

        let report = compare_database_snapshots_with_labels(
            &before,
            &after,
            "Mastodon before",
            "Mastodon after",
            DEFAULT_MISMATCH_LIMIT,
        )
        .expect_err("the test row intentionally changed");
        let diagnostic = report.to_string();
        assert!(diagnostic.contains("Mastodon before=\"before\""));
        assert!(diagnostic.contains("Mastodon after=\"after\""));
        assert!(!diagnostic.contains("Rust="));
    }

    #[test]
    fn oauth_database_mismatches_never_render_credentials() {
        let mastodon = database(table(
            "oauth_access_tokens",
            &["id"],
            vec![
                json!({"id": 1, "token": "fixture-changed-secret", "refresh_token": "fixture-refresh-secret"}),
                json!({"id": 2, "token": "fixture-deleted-secret", "refresh_token": null}),
            ],
        ));
        let rust = database(table(
            "oauth_access_tokens",
            &["id"],
            vec![
                json!({"id": 1, "token": "different-secret", "refresh_token": "different-refresh"}),
                json!({"id": 3, "token": "fixture-added-secret", "refresh_token": null}),
            ],
        ));

        let report = compare_database_snapshots(&mastodon, &rust, DEFAULT_MISMATCH_LIMIT)
            .expect_err("OAuth rows intentionally differ");
        let diagnostic = format!("{report:?}\n{report}");
        assert!(diagnostic.contains("[REDACTED]"));
        for fragment in [
            "fixture-changed",
            "fixture-refresh",
            "fixture-deleted",
            "fixture-added",
            "different-secret",
            "different-refresh",
        ] {
            assert!(!diagnostic.contains(fragment), "leaked {fragment:?}");
        }
    }

    #[test]
    fn all_known_credential_columns_are_redacted_from_whole_rows() {
        for (table_name, row) in [
            (
                "accounts",
                json!({"id": 1, "private_key": "fixture-sensitive-account"}),
            ),
            (
                "email_subscriptions",
                json!({"id": 1, "confirmation_token": "fixture-sensitive-email"}),
            ),
            (
                "fasp_providers",
                json!({"id": 1, "server_private_key_pem": "fixture-sensitive-fasp"}),
            ),
            (
                "generated_annual_reports",
                json!({"id": 1, "share_key": "fixture-sensitive-report"}),
            ),
            (
                "keypairs",
                json!({"id": 1, "private_key": "fixture-sensitive-keypair"}),
            ),
            (
                "oauth_access_grants",
                json!({"id": 1, "token": "fixture-sensitive-grant"}),
            ),
            (
                "oauth_access_tokens",
                json!({"id": 1, "token": "fixture-sensitive-access", "refresh_token": "fixture-sensitive-refresh"}),
            ),
            (
                "oauth_applications",
                json!({"id": 1, "secret": "fixture-sensitive-application"}),
            ),
            (
                "session_activations",
                json!({"id": 1, "session_id": "fixture-sensitive-session"}),
            ),
            (
                "users",
                json!({
                    "id": 1,
                    "confirmation_token": "fixture-sensitive-confirmation",
                    "encrypted_password": "fixture-sensitive-password",
                    "otp_backup_codes": ["fixture-sensitive-backup"],
                    "otp_secret": "fixture-sensitive-otp",
                    "reset_password_token": "fixture-sensitive-reset",
                    "sign_in_token": "fixture-sensitive-sign-in"
                }),
            ),
            (
                "web_push_subscriptions",
                json!({
                    "id": 1,
                    "data": {"auth": "fixture-sensitive-push-data"},
                    "endpoint": "fixture-sensitive-push-endpoint",
                    "key_auth": "fixture-sensitive-push-auth",
                    "key_p256dh": "fixture-sensitive-push-key"
                }),
            ),
            (
                "webhooks",
                json!({"id": 1, "secret": "fixture-sensitive-webhook"}),
            ),
        ] {
            let redacted = redact_observation(table_name, "$", &ObservedJson::Value(row));
            let diagnostic = format!("{redacted:?}");
            assert!(diagnostic.contains("[REDACTED]"), "{table_name}");
            assert!(!diagnostic.contains("fixture-sensitive"), "{table_name}");
        }
    }
}
