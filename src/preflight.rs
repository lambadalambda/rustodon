use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::TryStreamExt;
use rustix::fs::{Access, access};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::{Connection, Executor, PgConnection, Row};
use url::{Host, Url};

use crate::config::{
    Config, ExternalAuthProvider, ObjectStorageProvider, PostgresConnection, PostgresSslMode,
    RedisEndpoint, SmtpConfig,
};
use crate::crypto::{ActiveRecordEncryptionConfig, RsaKeyError, validate_rsa_signing_keypair};
use crate::secret::SecretString;

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const REDIS_IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PRIVATE_KEY_BYTES: usize = 64 * 1024;
const MIGRATIONS: &str = include_str!("../fixtures/mastodon/v4.6.5/migrations.tsv");
const CATALOG: &str = include_str!("../fixtures/mastodon/v4.6.5/catalog.txt");

pub(crate) const V1_CRITICAL_TABLES: &[&str] = &[
    "account_conversations",
    "account_deletion_requests",
    "account_domain_blocks",
    "account_notes",
    "account_pins",
    "account_relationship_severance_events",
    "account_stats",
    "account_statuses_cleanup_policies",
    "account_warnings",
    "accounts",
    "accounts_tags",
    "appeals",
    "blocks",
    "bookmarks",
    "collection_items",
    "collection_reports",
    "collections",
    "conversation_mutes",
    "conversations",
    "custom_emojis",
    "custom_filter_keywords",
    "custom_filter_statuses",
    "custom_filters",
    "domain_allows",
    "domain_blocks",
    "favourites",
    "featured_tags",
    "follow_requests",
    "follows",
    "generated_annual_reports",
    "instances",
    "keypairs",
    "list_accounts",
    "lists",
    "markers",
    "media_attachments",
    "mentions",
    "mutes",
    "notification_permissions",
    "notification_policies",
    "notification_requests",
    "notifications",
    "oauth_access_tokens",
    "oauth_applications",
    "poll_votes",
    "polls",
    "preview_card_providers",
    "preview_cards",
    "preview_cards_statuses",
    "quotes",
    "relays",
    "relationship_severance_events",
    "reports",
    "rule_translations",
    "rules",
    "scheduled_statuses",
    "settings",
    "site_uploads",
    "status_edits",
    "status_pins",
    "status_stats",
    "statuses",
    "statuses_tags",
    "tag_follows",
    "tagged_objects",
    "tags",
    "tombstones",
    "user_roles",
    "users",
    "webauthn_credentials",
];

const SNOWFLAKE_SEQUENCES: &[&str] = &[
    "accounts_id_seq",
    "collection_items_id_seq",
    "collections_id_seq",
    "media_attachments_id_seq",
    "notification_requests_id_seq",
    "quotes_id_seq",
    "statuses_id_seq",
];

const CATALOG_QUERY: &str = r#"
WITH catalog_entries AS (
  SELECT
    'relation' AS object_kind,
    c.relname AS object_name,
    json_build_object(
      'schema', n.nspname,
      'name', c.relname,
      'kind', c.relkind,
      'persistence', c.relpersistence,
      'partitioned', c.relispartition,
      'row_security', c.relrowsecurity
    )::text AS definition
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f')

  UNION ALL

  SELECT
    'column',
    c.relname || '.' || a.attname,
    json_build_object(
      'schema', n.nspname,
      'relation', c.relname,
      'position', a.attnum,
      'name', a.attname,
      'type', pg_catalog.format_type(a.atttypid, a.atttypmod),
      'not_null', a.attnotnull,
      'default', pg_get_expr(d.adbin, d.adrelid, true),
      'identity', a.attidentity,
      'generated', a.attgenerated,
      'collation', CASE WHEN a.attcollation = 0 THEN NULL ELSE coll.collname END
    )::text
  FROM pg_attribute a
  JOIN pg_class c ON c.oid = a.attrelid
  JOIN pg_namespace n ON n.oid = c.relnamespace
  LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
  LEFT JOIN pg_collation coll ON coll.oid = a.attcollation
  WHERE n.nspname = 'public'
    AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
    AND a.attnum > 0
    AND NOT a.attisdropped

  UNION ALL

  SELECT
    'constraint',
    c.relname || '.' || con.conname,
    json_build_object(
      'schema', n.nspname,
      'relation', c.relname,
      'name', con.conname,
      'type', con.contype,
      'deferrable', con.condeferrable,
      'initially_deferred', con.condeferred,
      'validated', con.convalidated,
      'definition', pg_get_constraintdef(con.oid, true)
    )::text
  FROM pg_constraint con
  JOIN pg_namespace n ON n.oid = con.connamespace
  JOIN pg_class c ON c.oid = con.conrelid
  WHERE n.nspname = 'public'

  UNION ALL

  SELECT
    'index',
    table_class.relname || '.' || index_class.relname,
    json_build_object(
      'schema', ns.nspname,
      'relation', table_class.relname,
      'name', index_class.relname,
      'unique', idx.indisunique,
      'primary', idx.indisprimary,
      'valid', idx.indisvalid,
      'ready', idx.indisready,
      'definition', pg_get_indexdef(idx.indexrelid, 0, true)
    )::text
  FROM pg_index idx
  JOIN pg_class table_class ON table_class.oid = idx.indrelid
  JOIN pg_class index_class ON index_class.oid = idx.indexrelid
  JOIN pg_namespace ns ON ns.oid = table_class.relnamespace
  WHERE ns.nspname = 'public'

  UNION ALL

  SELECT
    CASE WHEN c.relkind = 'm' THEN 'materialized_view' ELSE 'view' END,
    c.relname,
    json_build_object(
      'schema', n.nspname,
      'name', c.relname,
      'definition', pg_get_viewdef(c.oid, true)
    )::text
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname = 'public' AND c.relkind IN ('v', 'm')
)
SELECT object_kind, object_name, definition
FROM catalog_entries
ORDER BY object_kind COLLATE "C", object_name COLLATE "C", definition COLLATE "C"
"#;

const SEQUENCE_QUERY: &str = r#"
SELECT
  'sequence' AS object_kind,
  c.relname AS object_name,
  json_build_object(
    'schema', n.nspname,
    'name', c.relname,
    'data_type', pg_catalog.format_type(s.seqtypid, NULL),
    'start', s.seqstart,
    'increment', s.seqincrement,
    'minimum', s.seqmin,
    'maximum', s.seqmax,
    'cache', s.seqcache,
    'cycle', s.seqcycle
  )::text AS definition
FROM pg_sequence s
JOIN pg_class c ON c.oid = s.seqrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public'
ORDER BY c.relname COLLATE "C"
"#;

const TIMESTAMP_FUNCTION_QUERY: &str = r"
SELECT
  pg_get_function_identity_arguments(p.oid) AS identity_arguments,
  pg_get_function_result(p.oid) AS result,
  l.lanname AS language,
  p.prokind::text AS kind,
  p.provolatile::text AS volatility,
  p.proparallel::text AS parallel,
  p.prosecdef AS security_definer,
  p.proleakproof AS leakproof,
  p.proisstrict AS strict,
  p.proconfig AS config,
  p.prosrc AS body
FROM pg_proc p
JOIN pg_namespace n ON n.oid = p.pronamespace
JOIN pg_language l ON l.oid = p.prolang
WHERE n.nspname = 'public'
  AND p.proname = 'timestamp_id'
  AND pg_get_function_identity_arguments(p.oid) = 'table_name text'
";

const IDENTIFIER_QUERY: &str = r#"
SELECT source.table_name, source.row_id, source.column_name, source.value
FROM (
  SELECT 'accounts'::text AS table_name, account.id AS row_id,
         field.column_name, field.value
  FROM accounts account
  CROSS JOIN LATERAL (VALUES
    ('uri'::text, account.uri::text),
    ('url'::text, account.url::text),
    ('inbox_url'::text, account.inbox_url::text),
    ('outbox_url'::text, account.outbox_url::text),
    ('followers_url'::text, account.followers_url::text),
    ('following_url'::text, account.following_url::text),
    ('shared_inbox_url'::text, account.shared_inbox_url::text),
    ('featured_collection_url'::text, account.featured_collection_url::text),
    ('collections_url'::text, account.collections_url::text)
  ) field(column_name, value)
  WHERE account.domain IS NULL

  UNION ALL

  SELECT 'statuses', status.id, field.column_name, field.value
  FROM statuses status
  JOIN accounts account ON account.id = status.account_id AND account.domain IS NULL
  CROSS JOIN LATERAL (VALUES
    ('uri'::text, status.uri::text),
    ('url'::text, status.url::text)
  ) field(column_name, value)
  WHERE status.deleted_at IS NULL

  UNION ALL

  SELECT 'conversations', conversation.id, 'uri', conversation.uri::text
  FROM conversations conversation
  JOIN accounts account ON account.id = conversation.parent_account_id AND account.domain IS NULL

  UNION ALL

  SELECT 'keypairs', keypair.id, 'uri', keypair.uri::text
  FROM keypairs keypair
  JOIN accounts account ON account.id = keypair.account_id AND account.domain IS NULL
) source
WHERE source.value IS NOT NULL AND source.value <> ''
ORDER BY source.table_name COLLATE "C", source.row_id, source.column_name COLLATE "C"
"#;

const ACTIVE_CONDITIONS_QUERY: &str = r"
SELECT
  (SELECT count(*) FROM scheduled_statuses) AS scheduled_statuses,
  (SELECT count(*)
     FROM polls poll
     JOIN statuses status ON status.id = poll.status_id AND status.deleted_at IS NULL
     JOIN accounts account ON account.id = status.account_id AND account.domain IS NULL
    WHERE poll.expires_at IS NULL OR poll.expires_at >= CURRENT_TIMESTAMP) AS active_local_polls,
  (SELECT count(*) FROM account_deletion_requests) AS account_deletions,
  (SELECT CASE WHEN count(*) = 1 THEN 0::bigint ELSE 1::bigint END
     FROM user_roles WHERE id = -99) AS invalid_everyone_role,
  (SELECT count(*)
     FROM users user_record
     JOIN accounts account ON account.id = user_record.account_id
    WHERE user_record.approved = true
      AND user_record.disabled = false
      AND user_record.confirmed_at IS NOT NULL
      AND user_record.otp_required_for_login = false
      AND account.suspended_at IS NULL
      AND account.memorial = false
      AND account.moved_to_account_id IS NULL
      AND EXISTS (
        SELECT 1 FROM webauthn_credentials credential
        WHERE credential.user_id = user_record.id
      )) AS webauthn_only_users,
  (SELECT count(*) FROM relays WHERE state IN (1, 2)) AS active_relays,
  (SELECT count(*) FROM account_statuses_cleanup_policies WHERE enabled) AS cleanup_policies
";

const SIDEKIQ_COUNT_SCRIPT: &str = r"
local count = redis.call('zcard', 'retry')
count = count + redis.call('zcard', 'schedule')
local cursor = '0'
repeat
  local result = redis.call('scan', cursor, 'match', 'queue:*', 'count', 100)
  cursor = result[1]
  for _, key in ipairs(result[2]) do
    count = count + redis.call('llen', key)
  end
until cursor == '0'
cursor = '0'
repeat
  local result = redis.call('sscan', 'processes', cursor, 'count', 100)
  cursor = result[1]
  for _, process in ipairs(result[2]) do
    count = count + redis.call('exists', process)
    count = count + redis.call('hlen', process .. ':work')
  end
until cursor == '0'
return count
";

/// Severity of a preflight diagnostic.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    Fatal,
    Warning,
}

/// A stable, secret-free operator diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    severity: Severity,
    code: &'static str,
    message: String,
    hint: String,
}

impl Diagnostic {
    #[must_use]
    pub fn fatal(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            severity: Severity::Fatal,
            code,
            message: message.into(),
            hint: hint.into(),
        }
    }

    #[must_use]
    pub fn warning(
        code: &'static str,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            hint: hint.into(),
        }
    }

    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn hint(&self) -> &str {
        &self.hint
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Fatal => "FATAL",
            Severity::Warning => "WARNING",
        };
        write!(
            formatter,
            "[{severity} {}] {}\n  remediation: {}",
            self.code, self.message, self.hint
        )
    }
}

/// Complete deterministic preflight result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreflightReport {
    diagnostics: Vec<Diagnostic>,
}

impl PreflightReport {
    #[must_use]
    pub fn from_diagnostics(diagnostics: impl IntoIterator<Item = Diagnostic>) -> Self {
        let mut diagnostics = diagnostics.into_iter().collect::<Vec<_>>();
        diagnostics.sort_by(|left, right| {
            severity_rank(left.severity)
                .cmp(&severity_rank(right.severity))
                .then_with(|| left.code.cmp(right.code))
                .then_with(|| left.message.cmp(&right.message))
        });
        Self { diagnostics }
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity == Severity::Warning)
    }
}

impl fmt::Display for PreflightReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fatal = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Fatal)
            .count();
        let warning = self.diagnostics.len() - fatal;
        if fatal == 0 {
            write!(formatter, "preflight passed: {warning} warning")?;
        } else {
            write!(
                formatter,
                "preflight failed: {fatal} fatal, {warning} warning"
            )?;
        }
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n{diagnostic}")?;
        }
        Ok(())
    }
}

const fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Fatal => 0,
        Severity::Warning => 1,
    }
}

/// Physical catalog object classes covered by the v1 fingerprint.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CatalogKind {
    Relation,
    Column,
    Constraint,
    Index,
    Sequence,
    View,
}

impl CatalogKind {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Relation => "RELATION",
            Self::Column => "COLUMN",
            Self::Constraint => "CONSTRAINT",
            Self::Index => "INDEX",
            Self::Sequence => "SEQUENCE",
            Self::View => "VIEW",
        }
    }
}

/// One normalized row from the pinned `PostgreSQL` catalog query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    pub kind: CatalogKind,
    pub name: String,
    pub definition: String,
}

/// Returns the exact 588-version migration inventory.
#[must_use]
pub fn expected_migration_versions() -> &'static BTreeSet<String> {
    static EXPECTED: OnceLock<BTreeSet<String>> = OnceLock::new();
    EXPECTED.get_or_init(|| {
        MIGRATIONS
            .lines()
            .filter_map(|line| {
                line.split_once('\t').and_then(|(version, _)| {
                    version
                        .bytes()
                        .all(|byte| byte.is_ascii_digit())
                        .then(|| version.to_owned())
                })
            })
            .collect()
    })
}

/// Compares both directions of the migration-version set.
#[must_use]
pub fn compare_migration_versions(actual: &BTreeSet<String>) -> Vec<Diagnostic> {
    let expected = expected_migration_versions();
    let missing = expected.difference(actual).map(|version| {
        Diagnostic::fatal(
            "PF_DB_MIGRATION_MISSING",
            format!("required migration {version} is not recorded as applied"),
            "fully migrate Mastodon v4.6.5 before cutover",
        )
    });
    let unexpected = actual.difference(expected).map(|version| {
        Diagnostic::fatal(
            "PF_DB_MIGRATION_UNEXPECTED",
            format!("unsupported migration {version} is recorded as applied"),
            "restore the pinned Mastodon v4.6.5 schema or update Rustodon compatibility",
        )
    });
    missing.chain(unexpected).collect()
}

/// Returns the physical fingerprint for the 55 v1 tables and `schema_migrations`.
#[must_use]
pub fn expected_catalog() -> &'static [CatalogEntry] {
    static EXPECTED: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    EXPECTED.get_or_init(|| {
        parse_catalog(CATALOG)
            .into_iter()
            .filter(catalog_entry_is_scoped)
            .collect()
    })
}

/// Returns the seven exact `timestamp_id()` backing sequence definitions.
#[must_use]
pub fn expected_sequences() -> &'static [CatalogEntry] {
    static EXPECTED: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    EXPECTED.get_or_init(|| {
        parse_catalog(CATALOG)
            .into_iter()
            .filter(|entry| {
                entry.kind == CatalogKind::Sequence
                    && SNOWFLAKE_SEQUENCES.contains(&entry.name.as_str())
            })
            .collect()
    })
}

fn parse_catalog(contents: &str) -> Vec<CatalogEntry> {
    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let kind = match fields.next()? {
                "relation" => CatalogKind::Relation,
                "column" => CatalogKind::Column,
                "constraint" => CatalogKind::Constraint,
                "index" => CatalogKind::Index,
                "sequence" => CatalogKind::Sequence,
                "view" | "materialized_view" => CatalogKind::View,
                _ => return None,
            };
            Some(CatalogEntry {
                kind,
                name: fields.next()?.to_owned(),
                definition: fields.next()?.to_owned(),
            })
        })
        .collect()
}

fn catalog_entry_is_scoped(entry: &CatalogEntry) -> bool {
    if entry.kind == CatalogKind::Sequence {
        return false;
    }
    let relation = if matches!(entry.kind, CatalogKind::Relation | CatalogKind::View) {
        entry.name.as_str()
    } else {
        entry
            .name
            .split_once('.')
            .map_or("", |(relation, _)| relation)
    };
    relation == "schema_migrations"
        || V1_CRITICAL_TABLES.contains(&relation)
        || constraint_references_scoped_relation(entry)
}

fn constraint_references_scoped_relation(entry: &CatalogEntry) -> bool {
    if entry.kind != CatalogKind::Constraint {
        return false;
    }
    let Ok(definition) = serde_json::from_str::<Value>(&entry.definition) else {
        return false;
    };
    let Some(definition) = definition.get("definition").and_then(Value::as_str) else {
        return false;
    };
    V1_CRITICAL_TABLES.iter().any(|relation| {
        [
            format!("REFERENCES {relation}("),
            format!("REFERENCES public.{relation}("),
            format!("REFERENCES \"{relation}\"("),
            format!("REFERENCES public.\"{relation}\"("),
        ]
        .iter()
        .any(|reference| definition.contains(reference))
    })
}

/// Compares scoped catalog entries structurally, ignoring JSON presentation whitespace.
#[must_use]
pub fn compare_catalog(actual: &[CatalogEntry]) -> Vec<Diagnostic> {
    compare_catalog_entries(
        expected_catalog(),
        actual.iter().filter(|entry| catalog_entry_is_scoped(entry)),
    )
}

/// Compares the seven sequence shapes while ignoring unrelated `PostgreSQL` sequences.
#[must_use]
pub fn compare_sequences(actual: &[CatalogEntry]) -> Vec<Diagnostic> {
    compare_catalog_entries(
        expected_sequences(),
        actual.iter().filter(|entry| {
            entry.kind == CatalogKind::Sequence
                && SNOWFLAKE_SEQUENCES.contains(&entry.name.as_str())
        }),
    )
}

fn compare_catalog_entries<'a>(
    expected: &'a [CatalogEntry],
    actual: impl IntoIterator<Item = &'a CatalogEntry>,
) -> Vec<Diagnostic> {
    let actual = actual.into_iter().collect::<Vec<_>>();
    let mut diagnostics = Vec::new();

    for expected_entry in expected {
        let Some(actual_entry) = actual.iter().find(|actual_entry| {
            actual_entry.kind == expected_entry.kind && actual_entry.name == expected_entry.name
        }) else {
            diagnostics.push(catalog_diagnostic(
                expected_entry.kind,
                "MISSING",
                &expected_entry.name,
                "restore the pinned physical schema before cutover",
            ));
            continue;
        };
        if !definitions_equal(&expected_entry.definition, &actual_entry.definition) {
            diagnostics.push(changed_catalog_diagnostic(expected_entry, actual_entry));
        }
    }
    for actual_entry in actual {
        if !expected.iter().any(|expected_entry| {
            expected_entry.kind == actual_entry.kind && expected_entry.name == actual_entry.name
        }) {
            diagnostics.push(catalog_diagnostic(
                actual_entry.kind,
                "UNEXPECTED",
                &actual_entry.name,
                "remove the unsupported object or update the pinned compatibility baseline",
            ));
        }
    }
    diagnostics
}

fn definitions_equal(expected: &str, actual: &str) -> bool {
    serde_json::from_str::<Value>(expected)
        .ok()
        .zip(serde_json::from_str::<Value>(actual).ok())
        .is_some_and(|(expected, actual)| expected == actual)
}

fn changed_catalog_diagnostic(expected: &CatalogEntry, actual: &CatalogEntry) -> Diagnostic {
    let suffix = if expected.kind == CatalogKind::Column {
        let expected = serde_json::from_str::<Value>(&expected.definition).ok();
        let actual = serde_json::from_str::<Value>(&actual.definition).ok();
        if json_field_differs(expected.as_ref(), actual.as_ref(), "type") {
            "TYPE_CHANGED"
        } else if json_field_differs(expected.as_ref(), actual.as_ref(), "not_null") {
            "NULLABILITY_CHANGED"
        } else if json_field_differs(expected.as_ref(), actual.as_ref(), "default") {
            "DEFAULT_CHANGED"
        } else {
            "CHANGED"
        }
    } else {
        "CHANGED"
    };
    catalog_diagnostic(
        expected.kind,
        suffix,
        &expected.name,
        "restore the pinned object definition before cutover",
    )
}

fn json_field_differs(expected: Option<&Value>, actual: Option<&Value>, field: &str) -> bool {
    expected.and_then(|value| value.get(field)) != actual.and_then(|value| value.get(field))
}

fn catalog_diagnostic(
    kind: CatalogKind,
    suffix: &'static str,
    name: &str,
    hint: &'static str,
) -> Diagnostic {
    let code = match (kind, suffix) {
        (CatalogKind::Relation, "MISSING") => "PF_DB_RELATION_MISSING",
        (CatalogKind::Relation, "UNEXPECTED") => "PF_DB_RELATION_UNEXPECTED",
        (CatalogKind::Relation, _) => "PF_DB_RELATION_CHANGED",
        (CatalogKind::Column, "MISSING") => "PF_DB_COLUMN_MISSING",
        (CatalogKind::Column, "UNEXPECTED") => "PF_DB_COLUMN_UNEXPECTED",
        (CatalogKind::Column, "TYPE_CHANGED") => "PF_DB_COLUMN_TYPE_CHANGED",
        (CatalogKind::Column, "NULLABILITY_CHANGED") => "PF_DB_COLUMN_NULLABILITY_CHANGED",
        (CatalogKind::Column, "DEFAULT_CHANGED") => "PF_DB_COLUMN_DEFAULT_CHANGED",
        (CatalogKind::Column, _) => "PF_DB_COLUMN_CHANGED",
        (CatalogKind::Constraint, "MISSING") => "PF_DB_CONSTRAINT_MISSING",
        (CatalogKind::Constraint, "UNEXPECTED") => "PF_DB_CONSTRAINT_UNEXPECTED",
        (CatalogKind::Constraint, _) => "PF_DB_CONSTRAINT_CHANGED",
        (CatalogKind::Index, "MISSING") => "PF_DB_INDEX_MISSING",
        (CatalogKind::Index, "UNEXPECTED") => "PF_DB_INDEX_UNEXPECTED",
        (CatalogKind::Index, _) => "PF_DB_INDEX_CHANGED",
        (CatalogKind::Sequence, "MISSING") => "PF_DB_SEQUENCE_MISSING",
        (CatalogKind::Sequence, "UNEXPECTED") => "PF_DB_SEQUENCE_UNEXPECTED",
        (CatalogKind::Sequence, _) => "PF_DB_SEQUENCE_CHANGED",
        (CatalogKind::View, "MISSING") => "PF_DB_VIEW_MISSING",
        (CatalogKind::View, "UNEXPECTED") => "PF_DB_VIEW_UNEXPECTED",
        (CatalogKind::View, _) => "PF_DB_VIEW_CHANGED",
    };
    Diagnostic::fatal(
        code,
        format!(
            "{} {name} does not match Mastodon v4.6.5",
            kind.diagnostic_name()
        ),
        hint,
    )
}

/// Shape and source body of the database's `timestamp_id(text)` function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimestampFunction {
    pub identity_arguments: String,
    pub result: String,
    pub language: String,
    pub kind: String,
    pub volatility: String,
    pub parallel: String,
    pub security_definer: bool,
    pub leakproof: bool,
    pub strict: bool,
    pub config: Option<Vec<String>>,
    pub body: String,
}

/// Accepts only the pinned function shape, with formatting and random salt variation.
#[must_use]
pub fn validate_timestamp_function(function: &TimestampFunction) -> Vec<Diagnostic> {
    if function.identity_arguments != "table_name text"
        || function.result != "bigint"
        || function.language != "plpgsql"
        || function.kind != "f"
        || function.volatility != "v"
        || function.parallel != "u"
        || function.security_definer
        || function.leakproof
        || function.strict
        || function.config.is_some()
    {
        return vec![Diagnostic::fatal(
            "PF_DB_TIMESTAMP_ID_SHAPE",
            "timestamp_id(table_name text) has an unsupported function shape",
            "restore Mastodon v4.6.5's timestamp_id(text) function",
        )];
    }

    let expected = normalize_timestamp_body(
        r"
        DECLARE
          time_part bigint;
          sequence_base bigint;
          tail bigint;
        BEGIN
          time_part := (((date_part('epoch', now()) * 1000))::bigint << 16);
          sequence_base := (
            'x' || substr(
              md5(table_name || 'salt-varies' || time_part::text), 1, 4
            )
          )::bit(16)::bigint;
          tail := ((sequence_base + nextval(table_name || '_id_seq')) & 65535);
          RETURN time_part | tail;
        END
        ",
    );
    if normalize_timestamp_body(&function.body) != expected {
        return vec![Diagnostic::fatal(
            "PF_DB_TIMESTAMP_ID_BODY",
            "timestamp_id(table_name text) has unsupported behavior",
            "restore the canonical function body; only its salt may differ",
        )];
    }
    Vec::new()
}

fn normalize_timestamp_body(body: &str) -> Option<String> {
    let without_comments = strip_sql_line_comments(body);
    let mut normalized = remove_sql_whitespace(&without_comments);
    let prefix = "md5(table_name||'";
    let suffix = "'||time_part::text)";
    let start = normalized.find(prefix)? + prefix.len();
    let end = start + normalized[start..].find(suffix)?;
    if start == end || normalized[start..end].contains('\'') {
        return None;
    }
    normalized.replace_range(start..end, "<salt>");
    Some(normalized)
}

fn remove_sql_whitespace(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut characters = sql.chars().peekable();
    let mut in_string = false;
    while let Some(character) = characters.next() {
        if character == '\'' {
            output.push(character);
            if in_string && characters.peek() == Some(&'\'') {
                output.push(characters.next().expect("peeked escaped quote"));
            } else {
                in_string = !in_string;
            }
        } else if in_string || !character.is_ascii_whitespace() {
            output.push(character);
        }
    }
    output
}

fn strip_sql_line_comments(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut characters = sql.chars().peekable();
    let mut in_string = false;
    while let Some(character) = characters.next() {
        if character == '\'' {
            output.push(character);
            if in_string && characters.peek() == Some(&'\'') {
                output.push(characters.next().expect("peeked escaped quote"));
            } else {
                in_string = !in_string;
            }
        } else if !in_string && character == '-' && characters.peek() == Some(&'-') {
            characters.next();
            for comment_character in characters.by_ref() {
                if comment_character == '\n' {
                    output.push('\n');
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}

/// A persisted canonical identifier, with its value kept out of diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedIdentifier {
    pub table: String,
    pub row_id: i64,
    pub column: String,
    pub value: String,
}

/// Validates URL authorities against `WEB_DOMAIN` and tag URI authorities against `LOCAL_DOMAIN`.
#[must_use]
pub fn validate_canonical_domains(
    web_domain: &str,
    local_domain: &str,
    identifiers: &[PersistedIdentifier],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut verified_local_domain = false;
    for identifier in identifiers {
        match identifier_authority(&identifier.value) {
            Some((IdentifierKind::Web, authority))
                if authority.eq_ignore_ascii_case(web_domain) => {}
            Some((IdentifierKind::Web, _)) => diagnostics.push(identifier_diagnostic(
                "PF_DB_WEB_DOMAIN_MISMATCH",
                identifier,
                "set WEB_DOMAIN to the persisted canonical web authority or migrate identifiers",
            )),
            Some((IdentifierKind::Tag, authority))
                if authority.eq_ignore_ascii_case(local_domain) =>
            {
                verified_local_domain = true;
            }
            Some((IdentifierKind::Tag, _)) => diagnostics.push(identifier_diagnostic(
                "PF_DB_LOCAL_DOMAIN_MISMATCH",
                identifier,
                "set LOCAL_DOMAIN to the persisted tag authority or migrate identifiers",
            )),
            None => diagnostics.push(identifier_diagnostic(
                "PF_DB_CANONICAL_IDENTIFIER_INVALID",
                identifier,
                "repair or remove the invalid persisted canonical identifier",
            )),
        }
    }
    if !verified_local_domain {
        diagnostics.push(Diagnostic::warning(
            "PF_DB_LOCAL_DOMAIN_UNVERIFIED",
            "no persisted tag URI verified LOCAL_DOMAIN",
            "confirm LOCAL_DOMAIN from the previous Mastodon deployment configuration",
        ));
    }
    diagnostics
}

#[derive(Clone, Copy)]
enum IdentifierKind {
    Web,
    Tag,
}

fn identifier_authority(value: &str) -> Option<(IdentifierKind, String)> {
    if let Some(tag) = value.strip_prefix("tag:") {
        let (authority, specific) = tag.split_once(',')?;
        if authority.is_empty() || specific.is_empty() || parse_bare_authority(authority).is_none()
        {
            return None;
        }
        return Some((IdentifierKind::Tag, authority.to_ascii_lowercase()));
    }
    if has_invalid_percent_encoding(value) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.username() != "" || url.password().is_some()
    {
        return None;
    }
    Some((IdentifierKind::Web, url_authority(&url)?))
}

fn parse_bare_authority(value: &str) -> Option<String> {
    let url = Url::parse(&format!("https://{value}")).ok()?;
    if url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.username() != ""
        || url.password().is_some()
    {
        return None;
    }
    url_authority(&url)
}

fn url_authority(url: &Url) -> Option<String> {
    let host = match url.host()? {
        Host::Domain(domain) => domain.to_ascii_lowercase(),
        Host::Ipv4(address) => address.to_string(),
        Host::Ipv6(address) => format!("[{address}]"),
    };
    Some(
        url.port()
            .map_or(host.clone(), |port| format!("{host}:{port}")),
    )
}

fn has_invalid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    (0..bytes.len()).any(|index| {
        bytes[index] == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
    })
}

fn identifier_diagnostic(
    code: &'static str,
    identifier: &PersistedIdentifier,
    hint: &'static str,
) -> Diagnostic {
    Diagnostic::fatal(
        code,
        format!(
            "{} row {} column {} has an incompatible canonical identifier",
            identifier.table, identifier.row_id, identifier.column
        ),
        hint,
    )
}

/// Safe failure classes for account and keypair signing keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyFailure {
    MissingPrivate,
    MissingPublic,
    CorruptPrivate,
    CorruptPublic,
    Mismatch,
    Undecryptable,
    Signing,
    Verification,
}

/// Maps key validation failures to stable diagnostics without retaining key material.
#[must_use]
pub fn key_failure_diagnostic(table: &str, row_id: i64, failure: KeyFailure) -> Diagnostic {
    let code = match failure {
        KeyFailure::MissingPrivate => "PF_DB_KEY_MISSING_PRIVATE",
        KeyFailure::MissingPublic => "PF_DB_KEY_MISSING_PUBLIC",
        KeyFailure::CorruptPrivate => "PF_DB_KEY_CORRUPT_PRIVATE",
        KeyFailure::CorruptPublic => "PF_DB_KEY_CORRUPT_PUBLIC",
        KeyFailure::Mismatch => "PF_DB_KEY_MISMATCH",
        KeyFailure::Undecryptable => "PF_DB_KEY_UNDECRYPTABLE",
        KeyFailure::Signing => "PF_DB_KEY_SIGNING_FAILED",
        KeyFailure::Verification => "PF_DB_KEY_VERIFICATION_FAILED",
    };
    Diagnostic::fatal(
        code,
        format!("{table} row {row_id} has an unusable local signing key"),
        "restore matching signing key material from a known-good backup before cutover",
    )
}

/// Reports typed unsupported configuration and optional-service warnings.
#[must_use]
pub fn configuration_diagnostics(config: &Config) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for provider in &config.unsupported.object_storage {
        let (code, name) = match provider {
            ObjectStorageProvider::S3 => ("PF_CONFIG_OBJECT_STORAGE_S3", "S3"),
            ObjectStorageProvider::Swift => ("PF_CONFIG_OBJECT_STORAGE_SWIFT", "Swift"),
            ObjectStorageProvider::Azure => ("PF_CONFIG_OBJECT_STORAGE_AZURE", "Azure"),
        };
        diagnostics.push(Diagnostic::fatal(
            code,
            format!("{name} object storage is unsupported in v1"),
            "migrate media to PAPERCLIP_ROOT_PATH and disable object storage",
        ));
    }
    for provider in &config.unsupported.external_auth {
        let (code, name) = match provider {
            ExternalAuthProvider::Ldap => ("PF_CONFIG_AUTH_LDAP", "LDAP"),
            ExternalAuthProvider::Pam => ("PF_CONFIG_AUTH_PAM", "PAM"),
            ExternalAuthProvider::Cas => ("PF_CONFIG_AUTH_CAS", "CAS"),
            ExternalAuthProvider::Saml => ("PF_CONFIG_AUTH_SAML", "SAML"),
            ExternalAuthProvider::Oidc => ("PF_CONFIG_AUTH_OIDC", "OIDC"),
        };
        diagnostics.push(Diagnostic::fatal(
            code,
            format!("{name} authentication is unsupported in v1"),
            "disable the external authentication provider before cutover",
        ));
    }
    if config.unsupported.omniauth_only {
        diagnostics.push(Diagnostic::fatal(
            "PF_CONFIG_OMNIAUTH_ONLY",
            "OMNIAUTH_ONLY is unsupported in v1",
            "restore password login and disable OMNIAUTH_ONLY before cutover",
        ));
    }
    if config.unsupported.one_click_sso_login {
        diagnostics.push(Diagnostic::fatal(
            "PF_CONFIG_ONE_CLICK_SSO",
            "ONE_CLICK_SSO_LOGIN is unsupported in v1",
            "disable one-click SSO before cutover",
        ));
    }
    if matches!(config.smtp, SmtpConfig::Disabled { .. }) {
        diagnostics.push(Diagnostic::warning(
            "PF_CONFIG_SMTP_DISABLED",
            "SMTP is disabled; outbound email will be unavailable",
            "configure SMTP_SERVER if Rustodon must send email",
        ));
    }
    diagnostics
}

/// Runs all local and `PostgreSQL` checks without mutating Mastodon data or media.
pub async fn run(config: &Config) -> PreflightReport {
    let mut diagnostics = runtime_diagnostics(config).await;
    if config.sidekiq_redis.is_some() {
        diagnostics.extend(sidekiq_redis_diagnostics(config).await);
    }
    PreflightReport::from_diagnostics(diagnostics)
}

/// Runs runtime-safe configuration, media, database, domain, key, and workflow checks.
///
/// Unlike cutover preflight, runtime validation does not require Mastodon's Redis to remain
/// available after queues have been drained.
pub async fn runtime_diagnostics(config: &Config) -> Vec<Diagnostic> {
    let mut diagnostics = configuration_diagnostics(config);
    diagnostics.extend(media_root_diagnostics(config));
    match database_diagnostics(config).await {
        Ok(database) => diagnostics.extend(database),
        Err(diagnostic) => diagnostics.push(diagnostic),
    }
    diagnostics
}

#[derive(Clone)]
struct RedisConnection {
    host: String,
    port: u16,
    database: u32,
    username: Option<SecretString>,
    password: Option<SecretString>,
    tls: bool,
}

async fn sidekiq_redis_diagnostics(config: &Config) -> Vec<Diagnostic> {
    let Some(endpoint) = config.sidekiq_redis.as_ref() else {
        return Vec::new();
    };
    let Ok(connection) = redis_connection(endpoint) else {
        return vec![redis_inspection_diagnostic()];
    };
    let inspected = tokio::time::timeout(
        CONNECTION_TIMEOUT,
        tokio::task::spawn_blocking(move || inspect_sidekiq_redis(&connection)),
    )
    .await;
    match inspected {
        Ok(Ok(Ok((0, 0)))) => Vec::new(),
        Ok(Ok(Ok((active, dead)))) => {
            let mut diagnostics = Vec::new();
            if active > 0 {
                diagnostics.push(Diagnostic::fatal(
                    "PF_REDIS_SIDEKIQ_NOT_EMPTY",
                    format!("Mastodon Sidekiq has {active} queued, deferred, or active jobs"),
                    "stop Mastodon job producers and drain queues, retries, and scheduled jobs",
                ));
            }
            if dead > 0 {
                diagnostics.push(Diagnostic::warning(
                    "PF_REDIS_SIDEKIQ_DEAD_JOBS",
                    format!("Mastodon Sidekiq has {dead} dead jobs requiring review"),
                    "review dead jobs before cutover; do not delete them automatically",
                ));
            }
            diagnostics
        }
        Err(_) | Ok(Err(_) | Ok(Err(()))) => vec![redis_inspection_diagnostic()],
    }
}

fn redis_connection(endpoint: &RedisEndpoint) -> Result<RedisConnection, ()> {
    match endpoint {
        RedisEndpoint::Tcp {
            host,
            port,
            database,
            username,
            password,
        } => Ok(RedisConnection {
            host: host.trim_matches(['[', ']']).to_owned(),
            port: *port,
            database: *database,
            username: username.clone(),
            password: password.clone(),
            tls: false,
        }),
        RedisEndpoint::Url(secret) => {
            let url = Url::parse(secret.expose_secret()).map_err(|_| ())?;
            if !matches!(url.scheme(), "redis" | "rediss") || url.query().is_some() {
                return Err(());
            }
            let path = url.path().trim_start_matches('/');
            let database = if path.is_empty() {
                0
            } else {
                path.parse::<u32>().map_err(|_| ())?
            };
            let username = (!url.username().is_empty())
                .then(|| decode_url_component(url.username()))
                .transpose()?
                .map(SecretString::new);
            let password = url
                .password()
                .map(decode_url_component)
                .transpose()?
                .map(SecretString::new);
            Ok(RedisConnection {
                host: url.host_str().ok_or(())?.to_owned(),
                port: url.port().unwrap_or(6379),
                database,
                username,
                password,
                tls: url.scheme() == "rediss",
            })
        }
    }
}

fn decode_url_component(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = bytes.get(index + 1..=index + 2).ok_or(())?;
            let encoded = std::str::from_utf8(digits).map_err(|_| ())?;
            decoded.push(u8::from_str_radix(encoded, 16).map_err(|_| ())?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| ())
}

fn inspect_sidekiq_redis(connection: &RedisConnection) -> Result<(i64, i64), ()> {
    if connection.username.is_some() && connection.password.is_none() {
        return Err(());
    }
    let addresses = (connection.host.as_str(), connection.port)
        .to_socket_addrs()
        .map_err(|_| ())?;
    let stream = addresses
        .into_iter()
        .find_map(|address| TcpStream::connect_timeout(&address, REDIS_IO_TIMEOUT).ok())
        .ok_or(())?;
    stream
        .set_read_timeout(Some(REDIS_IO_TIMEOUT))
        .map_err(|_| ())?;
    stream
        .set_write_timeout(Some(REDIS_IO_TIMEOUT))
        .map_err(|_| ())?;
    let mut stream: Box<dyn RedisIo> = if connection.tls {
        let roots = webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .cloned()
            .collect::<RootCertStore>();
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server_name = ServerName::try_from(connection.host.clone()).map_err(|_| ())?;
        let client = ClientConnection::new(Arc::new(config), server_name).map_err(|_| ())?;
        Box::new(StreamOwned::new(client, stream))
    } else {
        Box::new(stream)
    };

    if let Some(password) = &connection.password {
        if let Some(username) = &connection.username {
            redis_command(
                &mut *stream,
                &[
                    b"AUTH",
                    username.expose_secret().as_bytes(),
                    password.expose_secret().as_bytes(),
                ],
            )?;
        } else {
            redis_command(
                &mut *stream,
                &[b"AUTH", password.expose_secret().as_bytes()],
            )?;
        }
        expect_redis_ok(&mut *stream)?;
    }
    if connection.database != 0 {
        let database = connection.database.to_string();
        redis_command(&mut *stream, &[b"SELECT", database.as_bytes()])?;
        expect_redis_ok(&mut *stream)?;
    }
    redis_command(
        &mut *stream,
        &[b"EVAL", SIDEKIQ_COUNT_SCRIPT.as_bytes(), b"0"],
    )?;
    let active = read_redis_integer(&mut *stream)?;
    redis_command(&mut *stream, &[b"ZCARD", b"dead"])?;
    let dead = read_redis_integer(&mut *stream)?;
    Ok((active, dead))
}

trait RedisIo: Read + Write {}

impl<T: Read + Write> RedisIo for T {}

fn redis_command(stream: &mut dyn RedisIo, arguments: &[&[u8]]) -> Result<(), ()> {
    write!(stream, "*{}\r\n", arguments.len()).map_err(|_| ())?;
    for argument in arguments {
        write!(stream, "${}\r\n", argument.len()).map_err(|_| ())?;
        stream.write_all(argument).map_err(|_| ())?;
        stream.write_all(b"\r\n").map_err(|_| ())?;
    }
    stream.flush().map_err(|_| ())
}

fn expect_redis_ok(stream: &mut dyn RedisIo) -> Result<(), ()> {
    let prefix = read_redis_byte(stream)?;
    let line = read_redis_line(stream)?;
    (prefix == b'+' && line == b"OK").then_some(()).ok_or(())
}

fn read_redis_integer(stream: &mut dyn RedisIo) -> Result<i64, ()> {
    if read_redis_byte(stream)? != b':' {
        return Err(());
    }
    let line = read_redis_line(stream)?;
    std::str::from_utf8(&line)
        .map_err(|_| ())?
        .parse::<i64>()
        .ok()
        .filter(|count| *count >= 0)
        .ok_or(())
}

fn read_redis_byte(stream: &mut dyn RedisIo) -> Result<u8, ()> {
    let mut byte = [0_u8; 1];
    stream.read_exact(&mut byte).map_err(|_| ())?;
    Ok(byte[0])
}

fn read_redis_line(stream: &mut dyn RedisIo) -> Result<Vec<u8>, ()> {
    let mut line = Vec::new();
    while line.len() <= 4096 {
        line.push(read_redis_byte(stream)?);
        if line.ends_with(b"\r\n") {
            line.truncate(line.len() - 2);
            return Ok(line);
        }
    }
    Err(())
}

fn redis_inspection_diagnostic() -> Diagnostic {
    Diagnostic::fatal(
        "PF_REDIS_SIDEKIQ_CHECK_FAILED",
        "Rustodon could not complete the read-only Sidekiq queue inspection",
        "verify the Redis endpoint, credentials, database, TLS mode, and network access",
    )
}

#[must_use]
pub fn media_root_diagnostics(config: &Config) -> Vec<Diagnostic> {
    let root = &config.paperclip.root_path;
    let Ok(canonical) = fs::canonicalize(root) else {
        return vec![Diagnostic::fatal(
            "PF_MEDIA_ROOT_UNREADABLE",
            "the configured Paperclip media root cannot be resolved",
            "create PAPERCLIP_ROOT_PATH and grant Rustodon directory access",
        )];
    };
    if canonical != *root {
        return vec![Diagnostic::fatal(
            "PF_MEDIA_ROOT_NOT_CANONICAL",
            "the configured Paperclip media root traverses a symlink or non-canonical component",
            "set PAPERCLIP_ROOT_PATH to the real absolute media directory",
        )];
    }
    let Ok(metadata) = fs::metadata(root) else {
        return vec![Diagnostic::fatal(
            "PF_MEDIA_ROOT_UNREADABLE",
            "the configured Paperclip media root cannot be read",
            "create PAPERCLIP_ROOT_PATH and grant Rustodon directory access",
        )];
    };
    if !metadata.is_dir() {
        return vec![Diagnostic::fatal(
            "PF_MEDIA_ROOT_NOT_DIRECTORY",
            "the configured Paperclip media root is not a directory",
            "set PAPERCLIP_ROOT_PATH to Mastodon's local media directory",
        )];
    }
    let mut diagnostics = Vec::new();
    if fs::read_dir(root).is_err() {
        diagnostics.push(Diagnostic::fatal(
            "PF_MEDIA_ROOT_UNREADABLE",
            "the configured Paperclip media root cannot be listed",
            "grant the Rustodon process read and search access to the media root",
        ));
    }
    if access(root, Access::READ_OK | Access::WRITE_OK | Access::EXEC_OK).is_err() {
        diagnostics.push(Diagnostic::fatal(
            "PF_MEDIA_ROOT_UNWRITABLE",
            "the configured Paperclip media root is not readable and writable by this process",
            "grant the Rustodon process write access without changing existing media",
        ));
    }
    diagnostics
}

async fn database_diagnostics(config: &Config) -> Result<Vec<Diagnostic>, Diagnostic> {
    let options = postgres_options(config).map_err(|_| database_connection_diagnostic())?;
    let mut connection =
        tokio::time::timeout(CONNECTION_TIMEOUT, PgConnection::connect_with(&options))
            .await
            .map_err(|_| database_connection_diagnostic())?
            .map_err(|_| database_connection_diagnostic())?;
    connection
        .execute("BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await
        .map_err(|_| database_inspection_diagnostic())?;
    sqlx::raw_sql(
        "SET LOCAL search_path TO pg_catalog, public, pg_temp; \
         SET LOCAL \"TimeZone\" TO 'UTC'; SET LOCAL lock_timeout TO '10s'; \
         SET LOCAL statement_timeout TO '60s'",
    )
    .execute(&mut connection)
    .await
    .map_err(|_| database_inspection_diagnostic())?;

    let result = inspect_database(&mut connection, config).await;
    let rollback = connection.execute("ROLLBACK").await;
    if rollback.is_err() {
        return Err(database_inspection_diagnostic());
    }
    result.map_err(|()| database_inspection_diagnostic())
}

#[derive(Debug)]
pub struct PostgresOptionsError;

impl std::fmt::Display for PostgresOptionsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid PostgreSQL connection URL")
    }
}

impl std::error::Error for PostgresOptionsError {}

/// Builds `SQLx` connection options from validated Mastodon configuration.
///
/// # Errors
///
/// Returns [`PostgresOptionsError`] when a configured `PostgreSQL` URL is invalid.
pub fn postgres_options(config: &Config) -> Result<PgConnectOptions, PostgresOptionsError> {
    let options = match &config.database.connection {
        PostgresConnection::Url { url, .. } => {
            PgConnectOptions::from_str(url.expose_secret()).map_err(|_| PostgresOptionsError)?
        }
        PostgresConnection::Tcp {
            host,
            port,
            database,
            username,
            password,
        } => PgConnectOptions::new()
            .host(host)
            .port(*port)
            .database(database)
            .username(username)
            .password(password.expose_secret()),
        PostgresConnection::UnixSocket {
            directory,
            port,
            database,
            username,
            password,
        } => PgConnectOptions::new()
            .socket(directory)
            .port(*port)
            .database(database)
            .username(username)
            .password(password.expose_secret()),
    };
    Ok(options.ssl_mode(match config.database.ssl_mode {
        PostgresSslMode::Disable => PgSslMode::Disable,
        PostgresSslMode::Allow => PgSslMode::Allow,
        PostgresSslMode::Prefer => PgSslMode::Prefer,
        PostgresSslMode::Require => PgSslMode::Require,
        PostgresSslMode::VerifyCa => PgSslMode::VerifyCa,
        PostgresSslMode::VerifyFull => PgSslMode::VerifyFull,
    }))
}

fn database_connection_diagnostic() -> Diagnostic {
    Diagnostic::fatal(
        "PF_DB_CONNECT",
        "Rustodon could not connect to PostgreSQL within the preflight timeout",
        "verify the primary database endpoint, credentials, TLS settings, and network access",
    )
}

fn database_inspection_diagnostic() -> Diagnostic {
    Diagnostic::fatal(
        "PF_DB_INSPECTION_FAILED",
        "PostgreSQL rejected a read-only preflight inspection",
        "grant SELECT and public-schema USAGE to the Rustodon database role",
    )
}

async fn inspect_database(
    connection: &mut PgConnection,
    config: &Config,
) -> Result<Vec<Diagnostic>, ()> {
    let migrations = sqlx::query_scalar::<_, String>(
        "SELECT version FROM schema_migrations ORDER BY version COLLATE \"C\"",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| ())?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let mut diagnostics = compare_migration_versions(&migrations);

    let catalog = fetch_catalog(connection, CATALOG_QUERY)
        .await
        .map_err(|_| ())?;
    let catalog_diagnostics = compare_catalog(&catalog);
    let physical_schema_matches = catalog_diagnostics.is_empty();
    diagnostics.extend(catalog_diagnostics);
    let physical_identity_diagnostics = fetch_physical_identity_diagnostics(connection)
        .await
        .map_err(|_| ())?;
    let physical_schema_matches =
        physical_schema_matches && physical_identity_diagnostics.is_empty();
    diagnostics.extend(physical_identity_diagnostics);

    let sequences = fetch_catalog(connection, SEQUENCE_QUERY)
        .await
        .map_err(|_| ())?;
    diagnostics.extend(compare_sequences(&sequences));
    diagnostics.extend(fetch_timestamp_function_diagnostics(connection).await?);

    if physical_schema_matches {
        diagnostics.extend(
            fetch_identifier_diagnostics(
                connection,
                &config.domains.web_domain,
                &config.domains.local_domain,
            )
            .await?,
        );
        diagnostics.extend(fetch_key_diagnostics(connection, config).await?);
        diagnostics.extend(fetch_active_condition_diagnostics(connection).await?);
    }
    Ok(diagnostics)
}

pub(crate) async fn validate_supported_mastodon_schema_in_transaction(
    connection: &mut PgConnection,
) -> Result<(), String> {
    sqlx::query(
        "SELECT pg_catalog.set_config('search_path', 'pg_catalog, public, pg_temp', true), \
                pg_catalog.set_config('TimeZone', 'UTC', true)",
    )
    .execute(&mut *connection)
    .await
    .map_err(|_| "could not secure the Mastodon schema inspection".to_owned())?;
    validate_safe_migration_environment(connection).await?;

    let relations = V1_CRITICAL_TABLES
        .iter()
        .filter(|name| **name != "instances")
        .chain(std::iter::once(&"schema_migrations"))
        .map(|name| format!("public.\"{}\"", name.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    sqlx::query(&format!(
        "LOCK TABLE {} IN ACCESS SHARE MODE",
        relations.join(", ")
    ))
    .execute(&mut *connection)
    .await
    .map_err(|error| format!("could not lock the Mastodon catalog boundary: {error}"))?;
    for sequence in SNOWFLAKE_SEQUENCES {
        sqlx::query(&format!(
            "SELECT last_value FROM public.\"{}\"",
            sequence.replace('"', "\"\"")
        ))
        .execute(&mut *connection)
        .await
        .map_err(|_| "could not lock the Mastodon sequence boundary".to_owned())?;
    }

    let migrations = sqlx::query_scalar::<_, String>(
        "SELECT version FROM public.schema_migrations ORDER BY version COLLATE \"C\"",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| "could not inspect Mastodon migrations".to_owned())?
    .into_iter()
    .collect::<BTreeSet<_>>();
    if !compare_migration_versions(&migrations).is_empty() {
        return Err("unsupported Mastodon migration inventory".to_owned());
    }
    let catalog = fetch_catalog(connection, CATALOG_QUERY)
        .await
        .map_err(|error| format!("could not inspect the Mastodon catalog: {error}"))?;
    if !compare_catalog(&catalog).is_empty() {
        return Err("unsupported Mastodon public catalog".to_owned());
    }
    let physical_identity_diagnostics = fetch_physical_identity_diagnostics(connection)
        .await
        .map_err(|error| format!("could not inspect Mastodon physical identities: {error}"))?;
    if !physical_identity_diagnostics.is_empty() {
        return Err("unsupported Mastodon collation or materialized-view state".to_owned());
    }
    let sequences = fetch_catalog(connection, SEQUENCE_QUERY)
        .await
        .map_err(|error| format!("could not inspect Mastodon sequences: {error}"))?;
    if !compare_sequences(&sequences).is_empty() {
        return Err("unsupported Mastodon sequence definitions".to_owned());
    }
    let timestamp_diagnostics = fetch_timestamp_function_diagnostics(connection)
        .await
        .map_err(|()| "could not inspect Mastodon's timestamp_id function".to_owned())?;
    if !timestamp_diagnostics.is_empty() {
        return Err("unsupported Mastodon timestamp_id function".to_owned());
    }
    let everyone_role_count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM public.user_roles WHERE id = -99")
            .fetch_one(&mut *connection)
            .await
            .map_err(|_| "could not inspect Mastodon's everyone role".to_owned())?;
    if everyone_role_count != 1 {
        return Err("Mastodon's mandatory everyone role is missing".to_owned());
    }
    Ok(())
}

async fn fetch_physical_identity_diagnostics(
    connection: &mut PgConnection,
) -> Result<Vec<Diagnostic>, sqlx::Error> {
    let relation_names = V1_CRITICAL_TABLES
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let collations = sqlx::query_scalar::<_, String>(
        "SELECT relation.relname || '.' || attribute.attname \
         FROM pg_catalog.pg_attribute attribute \
         JOIN pg_catalog.pg_class relation ON relation.oid = attribute.attrelid \
         JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
         JOIN pg_catalog.pg_collation collation_record \
           ON collation_record.oid = attribute.attcollation \
         JOIN pg_catalog.pg_namespace collation_namespace \
           ON collation_namespace.oid = collation_record.collnamespace \
         WHERE namespace.nspname = 'public' AND relation.relname = ANY($1) \
           AND attribute.attnum > 0 AND NOT attribute.attisdropped \
           AND collation_namespace.nspname <> 'pg_catalog' \
         ORDER BY relation.relname COLLATE \"C\", attribute.attname COLLATE \"C\"",
    )
    .bind(&relation_names)
    .fetch_all(&mut *connection)
    .await?;
    let instances_unpopulated = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
           SELECT 1 FROM pg_catalog.pg_class relation \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           WHERE namespace.nspname = 'public' AND relation.relname = 'instances' \
             AND relation.relkind = 'm' AND NOT relation.relispopulated)",
    )
    .fetch_one(&mut *connection)
    .await?;
    let mut diagnostics = collations
        .into_iter()
        .map(|name| {
            Diagnostic::fatal(
                "PF_DB_COLUMN_CHANGED",
                format!("COLUMN {name} does not match Mastodon v4.6.5"),
                "restore the pinned column collation before cutover",
            )
        })
        .collect::<Vec<_>>();
    if instances_unpopulated {
        diagnostics.push(Diagnostic::fatal(
            "PF_DB_RELATION_CHANGED",
            "RELATION instances does not match Mastodon v4.6.5",
            "populate the pinned instances materialized view before cutover",
        ));
    }
    Ok(diagnostics)
}

async fn validate_safe_migration_environment(connection: &mut PgConnection) -> Result<(), String> {
    let unsafe_ddl_environment = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
           SELECT 1 FROM pg_catalog.pg_event_trigger WHERE evtenabled <> 'D' \
           UNION ALL \
           SELECT 1 FROM pg_catalog.pg_publication WHERE puballtables)",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| "could not inspect PostgreSQL DDL hooks".to_owned())?;
    if unsafe_ddl_environment {
        return Err(
            "enabled event triggers or all-table publications are not supported".to_owned(),
        );
    }

    let relation_names = V1_CRITICAL_TABLES
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let unsafe_relation_behavior = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
           SELECT 1 FROM pg_catalog.pg_trigger trigger \
           JOIN pg_catalog.pg_class relation ON relation.oid = trigger.tgrelid \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           WHERE namespace.nspname = 'public' AND relation.relname = ANY($1) \
             AND NOT trigger.tgisinternal \
           UNION ALL \
           SELECT 1 FROM pg_catalog.pg_rewrite rule \
           JOIN pg_catalog.pg_class relation ON relation.oid = rule.ev_class \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           WHERE namespace.nspname = 'public' AND relation.relname = ANY($1) \
             AND NOT (relation.relkind IN ('v', 'm') AND rule.rulename = '_RETURN') \
           UNION ALL \
           SELECT 1 FROM pg_catalog.pg_policy policy \
           JOIN pg_catalog.pg_class relation ON relation.oid = policy.polrelid \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           WHERE namespace.nspname = 'public' AND relation.relname = ANY($1))",
    )
    .bind(&relation_names)
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| "could not inspect Mastodon relation behavior".to_owned())?;
    if unsafe_relation_behavior {
        return Err(
            "unsupported triggers, rules, or policies affect Mastodon relations".to_owned(),
        );
    }
    Ok(())
}

async fn fetch_catalog(
    connection: &mut PgConnection,
    query: &str,
) -> Result<Vec<CatalogEntry>, String> {
    sqlx::query(query)
        .fetch_all(connection)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| {
            let kind = match row
                .try_get::<String, _>("object_kind")
                .map_err(|error| error.to_string())?
                .as_str()
            {
                "relation" => CatalogKind::Relation,
                "column" => CatalogKind::Column,
                "constraint" => CatalogKind::Constraint,
                "index" => CatalogKind::Index,
                "sequence" => CatalogKind::Sequence,
                "view" | "materialized_view" => CatalogKind::View,
                _ => return Err("unsupported catalog entry kind".to_owned()),
            };
            Ok(CatalogEntry {
                kind,
                name: row
                    .try_get("object_name")
                    .map_err(|error| error.to_string())?,
                definition: row
                    .try_get("definition")
                    .map_err(|error| error.to_string())?,
            })
        })
        .collect()
}

async fn fetch_timestamp_function_diagnostics(
    connection: &mut PgConnection,
) -> Result<Vec<Diagnostic>, ()> {
    let Some(row) = sqlx::query(TIMESTAMP_FUNCTION_QUERY)
        .fetch_optional(connection)
        .await
        .map_err(|_| ())?
    else {
        return Ok(vec![Diagnostic::fatal(
            "PF_DB_TIMESTAMP_ID_MISSING",
            "timestamp_id(table_name text) is missing",
            "restore Mastodon v4.6.5's timestamp_id(text) function",
        )]);
    };
    let function = TimestampFunction {
        identity_arguments: row.try_get("identity_arguments").map_err(|_| ())?,
        result: row.try_get("result").map_err(|_| ())?,
        language: row.try_get("language").map_err(|_| ())?,
        kind: row.try_get("kind").map_err(|_| ())?,
        volatility: row.try_get("volatility").map_err(|_| ())?,
        parallel: row.try_get("parallel").map_err(|_| ())?,
        security_definer: row.try_get("security_definer").map_err(|_| ())?,
        leakproof: row.try_get("leakproof").map_err(|_| ())?,
        strict: row.try_get("strict").map_err(|_| ())?,
        config: row.try_get("config").map_err(|_| ())?,
        body: row.try_get("body").map_err(|_| ())?,
    };
    Ok(validate_timestamp_function(&function))
}

async fn fetch_identifier_diagnostics(
    connection: &mut PgConnection,
    web_domain: &str,
    local_domain: &str,
) -> Result<Vec<Diagnostic>, ()> {
    let mut diagnostics = Vec::new();
    let mut verified_local_domain = false;
    let mut rows = sqlx::query(IDENTIFIER_QUERY).fetch(connection);
    while let Some(row) = rows.try_next().await.map_err(|_| ())? {
        let identifier = PersistedIdentifier {
            table: row.try_get("table_name").map_err(|_| ())?,
            row_id: row.try_get("row_id").map_err(|_| ())?,
            column: row.try_get("column_name").map_err(|_| ())?,
            value: row.try_get("value").map_err(|_| ())?,
        };
        match identifier_authority(&identifier.value) {
            Some((IdentifierKind::Web, authority))
                if authority.eq_ignore_ascii_case(web_domain) => {}
            Some((IdentifierKind::Web, _)) if diagnostics.len() < 100 => {
                diagnostics.push(identifier_diagnostic(
                    "PF_DB_WEB_DOMAIN_MISMATCH",
                    &identifier,
                    "set WEB_DOMAIN to the persisted canonical web authority or migrate identifiers",
                ));
            }
            Some((IdentifierKind::Tag, authority))
                if authority.eq_ignore_ascii_case(local_domain) =>
            {
                verified_local_domain = true;
            }
            Some((IdentifierKind::Tag, _)) if diagnostics.len() < 100 => {
                diagnostics.push(identifier_diagnostic(
                    "PF_DB_LOCAL_DOMAIN_MISMATCH",
                    &identifier,
                    "set LOCAL_DOMAIN to the persisted tag authority or migrate identifiers",
                ));
            }
            None if diagnostics.len() < 100 => diagnostics.push(identifier_diagnostic(
                "PF_DB_CANONICAL_IDENTIFIER_INVALID",
                &identifier,
                "repair or remove the invalid persisted canonical identifier",
            )),
            Some(_) | None => {}
        }
    }
    if !verified_local_domain {
        diagnostics.push(Diagnostic::warning(
            "PF_DB_LOCAL_DOMAIN_UNVERIFIED",
            "no persisted tag URI verified LOCAL_DOMAIN",
            "confirm LOCAL_DOMAIN from the previous Mastodon deployment configuration",
        ));
    }
    Ok(diagnostics)
}

async fn fetch_key_diagnostics(
    connection: &mut PgConnection,
    config: &Config,
) -> Result<Vec<Diagnostic>, ()> {
    let mut diagnostics = Vec::new();
    {
        let mut account_rows = sqlx::query(
            "SELECT id, private_key, public_key FROM accounts WHERE domain IS NULL ORDER BY id",
        )
        .fetch(&mut *connection);
        while let Some(row) = account_rows.try_next().await.map_err(|_| ())? {
            let row_id = row.try_get("id").map_err(|_| ())?;
            let private_key = row
                .try_get::<Option<String>, _>("private_key")
                .map_err(|_| ())?
                .map(SecretString::new);
            let public_key = row.try_get::<String, _>("public_key").map_err(|_| ())?;
            if let Err(error) = validate_rsa_signing_keypair(private_key.as_ref(), &public_key) {
                diagnostics.push(key_failure_diagnostic(
                    "accounts",
                    row_id,
                    key_failure_from_rsa(error),
                ));
            }
        }
    }

    let encryption = ActiveRecordEncryptionConfig::new(
        config.secrets.active_record_encryption.primary_key.clone(),
        config
            .secrets
            .active_record_encryption
            .deterministic_key
            .clone(),
        config
            .secrets
            .active_record_encryption
            .key_derivation_salt
            .clone(),
    )
    .map_err(|_| ())?;
    let mut keypair_rows = sqlx::query(
        "SELECT keypair.id, keypair.private_key, keypair.public_key \
         FROM keypairs keypair \
         JOIN accounts account ON account.id = keypair.account_id AND account.domain IS NULL \
         WHERE keypair.revoked = false \
           AND (keypair.expires_at IS NULL OR keypair.expires_at > CURRENT_TIMESTAMP) \
         ORDER BY keypair.id",
    )
    .fetch(&mut *connection);
    while let Some(row) = keypair_rows.try_next().await.map_err(|_| ())? {
        let row_id = row.try_get("id").map_err(|_| ())?;
        let serialized = row
            .try_get::<Option<String>, _>("private_key")
            .map_err(|_| ())?;
        let public_key = row.try_get::<String, _>("public_key").map_err(|_| ())?;
        let private_key = match serialized {
            Some(value) => {
                let Ok(value) = encryption.decrypt_string(&value, MAX_PRIVATE_KEY_BYTES) else {
                    diagnostics.push(key_failure_diagnostic(
                        "keypairs",
                        row_id,
                        KeyFailure::Undecryptable,
                    ));
                    continue;
                };
                Some(value)
            }
            None => None,
        };
        if let Err(error) = validate_rsa_signing_keypair(private_key.as_ref(), &public_key) {
            diagnostics.push(key_failure_diagnostic(
                "keypairs",
                row_id,
                key_failure_from_rsa(error),
            ));
        }
    }
    Ok(diagnostics)
}

const fn key_failure_from_rsa(error: RsaKeyError) -> KeyFailure {
    match error {
        RsaKeyError::MissingPrivateKey => KeyFailure::MissingPrivate,
        RsaKeyError::MissingPublicKey => KeyFailure::MissingPublic,
        RsaKeyError::CorruptPrivateKey => KeyFailure::CorruptPrivate,
        RsaKeyError::CorruptPublicKey => KeyFailure::CorruptPublic,
        RsaKeyError::KeyMismatch => KeyFailure::Mismatch,
        RsaKeyError::SigningFailed => KeyFailure::Signing,
        RsaKeyError::SignatureVerificationFailed => KeyFailure::Verification,
    }
}

async fn fetch_active_condition_diagnostics(
    connection: &mut PgConnection,
) -> Result<Vec<Diagnostic>, ()> {
    let row = sqlx::query(ACTIVE_CONDITIONS_QUERY)
        .fetch_one(connection)
        .await
        .map_err(|_| ())?;
    let conditions = [
        (
            row.try_get::<i64, _>("scheduled_statuses")
                .map_err(|_| ())?,
            "PF_DB_SCHEDULED_STATUSES_PENDING",
            "scheduled statuses are pending",
            "publish or cancel every scheduled status before cutover",
        ),
        (
            row.try_get::<i64, _>("active_local_polls")
                .map_err(|_| ())?,
            "PF_DB_LOCAL_POLLS_ACTIVE",
            "local polls are still active",
            "wait for local polls to close before cutover",
        ),
        (
            row.try_get::<i64, _>("account_deletions").map_err(|_| ())?,
            "PF_DB_ACCOUNT_DELETIONS_PENDING",
            "account deletion requests are pending",
            "finish or cancel every account deletion before cutover",
        ),
        (
            row.try_get::<i64, _>("invalid_everyone_role")
                .map_err(|_| ())?,
            "PF_DB_EVERYONE_ROLE_INVALID",
            "the mandatory everyone role is missing",
            "restore the Mastodon everyone role with ID -99 before cutover",
        ),
        (
            row.try_get::<i64, _>("webauthn_only_users")
                .map_err(|_| ())?,
            "PF_DB_WEBAUTHN_ONLY_USERS",
            "users depend exclusively on unsupported WebAuthn credentials",
            "configure a supported login and recovery path for every affected user",
        ),
        (
            row.try_get::<i64, _>("active_relays").map_err(|_| ())?,
            "PF_DB_RELAYS_ACTIVE",
            "federation relays are active",
            "disable every federation relay before cutover",
        ),
        (
            row.try_get::<i64, _>("cleanup_policies").map_err(|_| ())?,
            "PF_DB_STATUS_CLEANUP_ENABLED",
            "automated status cleanup policies are enabled",
            "disable automated status cleanup before cutover",
        ),
    ];
    Ok(conditions
        .into_iter()
        .filter(|(count, _, _, _)| *count > 0)
        .map(|(_, code, message, hint)| Diagnostic::fatal(code, message, hint))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use crate::config::RedisEndpoint;
    use crate::secret::SecretString;

    use super::{RedisConnection, inspect_sidekiq_redis, redis_connection};

    #[test]
    fn rediss_urls_select_the_tls_transport_without_exposing_credentials() {
        let endpoint = RedisEndpoint::Url(SecretString::new(
            "rediss://queue-user:queue-secret@redis.example/3".to_owned(),
        ));
        let connection = redis_connection(&endpoint).unwrap();
        assert!(connection.tls);
        assert_eq!(connection.host, "redis.example");
        assert_eq!(connection.port, 6379);
        assert_eq!(connection.database, 3);
        assert_eq!(connection.username.unwrap().expose_secret(), "queue-user");
        assert_eq!(connection.password.unwrap().expose_secret(), "queue-secret");
    }

    #[test]
    fn read_only_sidekiq_protocol_returns_the_server_job_count() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n0\r\n") {
                let mut chunk = [0_u8; 1024];
                let length = std::io::Read::read(&mut stream, &mut chunk).unwrap();
                assert!(length > 0);
                request.extend_from_slice(&chunk[..length]);
                assert!(request.len() <= 16 * 1024);
            }
            assert!(request.windows(4).any(|window| window == b"EVAL"));
            assert!(request.windows(7).any(|window| window == b"queue:*"));
            std::io::Write::write_all(&mut stream, b":4\r\n").unwrap();
            request.clear();
            while !request.ends_with(b"\r\ndead\r\n") {
                let mut chunk = [0_u8; 1024];
                let length = std::io::Read::read(&mut stream, &mut chunk).unwrap();
                assert!(length > 0);
                request.extend_from_slice(&chunk[..length]);
                assert!(request.len() <= 1024);
            }
            std::io::Write::write_all(&mut stream, b":2\r\n").unwrap();
        });
        let connection = RedisConnection {
            host: address.ip().to_string(),
            port: address.port(),
            database: 0,
            username: None,
            password: None,
            tls: false,
        };

        assert_eq!(inspect_sidekiq_redis(&connection), Ok((4, 2)));
        server.join().unwrap();
    }
}
