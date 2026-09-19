use std::fmt;
use std::path::{Path, PathBuf};

use bcrypt::{DEFAULT_COST, hash, verify};
use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey};
use rsa::pkcs8::{DecodePublicKey, EncodePublicKey, LineEnding};
use rsa::rand_core::{OsRng, RngCore};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, Postgres, Transaction};

use crate::{operational_schema, paperclip::PaperclipRoot, preflight};

const SCHEMA_SQL: &str = include_str!("../migrations/mastodon/v4.6.5/public-schema.sql");
const MIGRATIONS_TSV: &str = include_str!("../fixtures/mastodon/v4.6.5/migrations.tsv");
const WRITER_GRANTS_SQL: &str = include_str!("../docs/mastodon-writer-grants.sql");
const SCHEMA_SALT_SENTINEL: &str = "__RUSTODON_TIMESTAMP_ID_SALT__";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseState {
    Fresh,
    Complete,
    Conflicting,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapInputs {
    pub admin_username: String,
    pub admin_email: String,
    pub site_title: String,
    pub runtime_role: String,
    pub writer_role: String,
    pub media_root: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapInputError(&'static str);

impl fmt::Display for BootstrapInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for BootstrapInputError {}

#[derive(Clone, Copy, Debug)]
pub struct BootstrapRequest<'a> {
    pub admin_username: &'a str,
    pub admin_email: &'a str,
    pub admin_password: &'a str,
    pub site_title: &'a str,
    pub runtime_role: &'a str,
    pub writer_role: &'a str,
    pub media_root: &'a Path,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapOutcome {
    Installed,
    Verified,
}

#[derive(Debug)]
pub enum BootstrapError {
    Input(BootstrapInputError),
    Io(std::io::Error),
    Database(sqlx::Error),
    Operational(operational_schema::MigrationError),
    Conflict(&'static str),
    Validation(String),
    Cryptographic(&'static str),
}

impl fmt::Display for BootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(error) => write!(formatter, "invalid bootstrap input: {error}"),
            Self::Io(_) => formatter.write_str("media root validation failed"),
            Self::Database(_) => formatter.write_str("database bootstrap failed"),
            Self::Operational(error) => {
                write!(formatter, "operational schema bootstrap failed: {error}")
            }
            Self::Conflict(message) => {
                write!(formatter, "bootstrap refused conflicting state: {message}")
            }
            Self::Validation(message) => {
                write!(formatter, "bootstrap validation failed: {message}")
            }
            Self::Cryptographic(message) => {
                write!(formatter, "bootstrap cryptography failed: {message}")
            }
        }
    }
}

impl std::error::Error for BootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Input(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::Operational(error) => Some(error),
            Self::Conflict(_) | Self::Validation(_) | Self::Cryptographic(_) => None,
        }
    }
}

impl From<BootstrapInputError> for BootstrapError {
    fn from(error: BootstrapInputError) -> Self {
        Self::Input(error)
    }
}

impl From<std::io::Error> for BootstrapError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<sqlx::Error> for BootstrapError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<operational_schema::MigrationError> for BootstrapError {
    fn from(error: operational_schema::MigrationError) -> Self {
        Self::Operational(error)
    }
}

#[must_use]
fn classify_database_state(public_initialized: bool, rustodon_exists: bool) -> DatabaseState {
    match (public_initialized, rustodon_exists) {
        (false, false) => DatabaseState::Fresh,
        (true, true) => DatabaseState::Complete,
        _ => DatabaseState::Conflicting,
    }
}

fn prepared_schema_sql(salt: &str) -> Result<String, BootstrapInputError> {
    if salt.len() != 32
        || !salt
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(BootstrapInputError(
            "schema salt must be exactly 32 lowercase hexadecimal characters",
        ));
    }
    if SCHEMA_SQL.matches(SCHEMA_SALT_SENTINEL).count() != 1
        || SCHEMA_SQL.matches("SET statement_timeout = 0;").count() != 1
        || SCHEMA_SQL.matches("SET lock_timeout = 0;").count() != 1
    {
        return Err(BootstrapInputError(
            "pinned schema bootstrap sentinels are invalid",
        ));
    }
    Ok(SCHEMA_SQL
        .replacen(
            "SET statement_timeout = 0;",
            "SET LOCAL statement_timeout TO '5min';",
            1,
        )
        .replacen(
            "SET lock_timeout = 0;",
            "SET LOCAL lock_timeout TO '10s';",
            1,
        )
        .replacen(SCHEMA_SALT_SENTINEL, salt, 1))
}

fn validate_inputs(
    admin_username: &str,
    admin_email: &str,
    password: &str,
    site_title: &str,
    runtime_role: &str,
    writer_role: &str,
    media_root: &Path,
) -> Result<BootstrapInputs, BootstrapInputError> {
    let admin_username = admin_username.trim().to_ascii_lowercase();
    if !(1..=30).contains(&admin_username.len())
        || !admin_username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(BootstrapInputError(
            "admin username must contain only letters, numbers, or underscores and be at most 30 bytes",
        ));
    }
    if reserved_username(&admin_username) {
        return Err(BootstrapInputError("admin username is reserved"));
    }

    let admin_email = admin_email.trim().to_ascii_lowercase();
    if admin_email.is_empty()
        || admin_email.len() > 320
        || admin_email.chars().any(char::is_whitespace)
        || admin_email.parse::<lettre::Address>().is_err()
    {
        return Err(BootstrapInputError("admin email must be a valid address"));
    }
    if !(8..=72).contains(&password.len()) {
        return Err(BootstrapInputError(
            "admin password must contain between 8 and 72 bytes",
        ));
    }

    let site_title = site_title.trim();
    if site_title.is_empty()
        || site_title.chars().count() > 120
        || site_title.chars().any(char::is_control)
    {
        return Err(BootstrapInputError(
            "site title must contain between 1 and 120 printable characters",
        ));
    }
    let runtime_role = runtime_role.trim();
    let writer_role = writer_role.trim();
    if runtime_role.is_empty() || writer_role.is_empty() || runtime_role == writer_role {
        return Err(BootstrapInputError(
            "runtime and writer roles must be nonempty and distinct",
        ));
    }
    if !media_root.is_absolute() {
        return Err(BootstrapInputError("media root must be an absolute path"));
    }

    Ok(BootstrapInputs {
        admin_username,
        admin_email,
        site_title: site_title.to_owned(),
        runtime_role: runtime_role.to_owned(),
        writer_role: writer_role.to_owned(),
        media_root: media_root.to_owned(),
    })
}

fn migration_versions() -> Result<Vec<String>, BootstrapInputError> {
    let mut lines = MIGRATIONS_TSV.lines();
    if lines.next() != Some("version\tphase\tpath\tsha256") {
        return Err(BootstrapInputError(
            "pinned migration inventory header is invalid",
        ));
    }
    let mut previous = None;
    let mut versions = Vec::new();
    for line in lines {
        let mut fields = line.split('\t');
        let version = fields.next().unwrap_or_default();
        if version.len() != 14
            || !version.bytes().all(|byte| byte.is_ascii_digit())
            || fields.clone().count() != 3
            || previous.is_some_and(|previous| previous >= version)
        {
            return Err(BootstrapInputError("pinned migration inventory is invalid"));
        }
        previous = Some(version);
        versions.push(version.to_owned());
    }
    if versions.len() != 588 {
        return Err(BootstrapInputError(
            "pinned migration inventory count is invalid",
        ));
    }
    Ok(versions)
}

fn site_title_yaml(title: &str) -> String {
    format!("--- '{}'\n", title.replace('\'', "''"))
}

fn prepared_writer_grants(
    quoted_writer_role: &str,
    quoted_database: &str,
) -> Result<String, BootstrapInputError> {
    let mut output = String::new();
    let mut format_blocks = 0_u8;
    let mut skipping_format = false;
    for line in WRITER_GRANTS_SQL.lines() {
        if skipping_format {
            if line.ends_with(") \\gexec") {
                skipping_format = false;
                format_blocks = format_blocks.saturating_add(1);
            }
            continue;
        }
        if line.starts_with("SELECT format(") {
            if format_blocks == 0 {
                use std::fmt::Write as _;
                writeln!(
                    output,
                    "REVOKE ALL PRIVILEGES ON DATABASE {quoted_database} FROM PUBLIC;"
                )
                .expect("writing to String is infallible");
                writeln!(
                    output,
                    "REVOKE ALL PRIVILEGES ON DATABASE {quoted_database} FROM {quoted_writer_role};"
                )
                .expect("writing to String is infallible");
                writeln!(
                    output,
                    "GRANT CONNECT ON DATABASE {quoted_database} TO {quoted_writer_role};"
                )
                .expect("writing to String is infallible");
            }
            skipping_format = true;
            continue;
        }
        if line.starts_with("\\set ") || line == "BEGIN;" || line == "COMMIT;" {
            continue;
        }
        output.push_str(&line.replace(":\"writer_role\"", quoted_writer_role));
        output.push('\n');
    }
    if skipping_format
        || format_blocks != 3
        || output.contains("\\gexec")
        || output.contains(":\"writer_role\"")
    {
        return Err(BootstrapInputError(
            "pinned writer grant script shape is invalid",
        ));
    }
    Ok(output)
}

fn reserved_username(username: &str) -> bool {
    const EXACT: &[&str] = &[
        "abuse",
        "account",
        "accounts",
        "admin",
        "administration",
        "administrator",
        "admins",
        "help",
        "helpdesk",
        "instance",
        "mod",
        "moderator",
        "moderators",
        "mods",
        "owner",
        "root",
        "security",
        "server",
        "staff",
        "support",
        "webmaster",
    ];
    EXACT.contains(&username) || username.contains("mastodon") || username.contains("mastadon")
}

/// Installs or verifies a standalone Rustodon baseline in an existing database and media root.
///
/// # Errors
///
/// Returns an error without committing when the target is unsupported, unsafe, partial, or drifted.
pub async fn bootstrap_instance(
    connection: &mut PgConnection,
    request: &BootstrapRequest<'_>,
) -> Result<BootstrapOutcome, BootstrapError> {
    let inputs = validate_inputs(
        request.admin_username,
        request.admin_email,
        request.admin_password,
        request.site_title,
        request.runtime_role,
        request.writer_role,
        request.media_root,
    )?;
    validate_media_root(&inputs.media_root)?;

    let mut transaction = connection.begin().await?;
    sqlx::raw_sql(
        "SET LOCAL lock_timeout TO '10s'; SET LOCAL statement_timeout TO '5min'; \
         SELECT pg_catalog.pg_advisory_xact_lock( \
           pg_catalog.hashtext(pg_catalog.current_database()), \
           pg_catalog.hashtext('rustodon:standalone-bootstrap'));",
    )
    .execute(&mut *transaction)
    .await?;
    validate_installer(&mut transaction).await?;
    validate_role(&mut transaction, &inputs.runtime_role, true).await?;
    validate_role(&mut transaction, &inputs.writer_role, false).await?;

    let state = database_state(&mut transaction).await?;
    match state {
        DatabaseState::Fresh => {
            validate_database_envelope(&mut transaction, false).await?;
            validate_pristine_database(&mut transaction).await?;
            install_fresh(&mut transaction, &inputs, request.admin_password).await?;
            transaction.commit().await?;
            Ok(BootstrapOutcome::Installed)
        }
        DatabaseState::Complete => {
            validate_database_envelope(&mut transaction, true).await?;
            validate_complete(&mut transaction, &inputs, request.admin_password).await?;
            transaction.rollback().await?;
            Ok(BootstrapOutcome::Verified)
        }
        DatabaseState::Conflicting => {
            transaction.rollback().await?;
            Err(BootstrapError::Conflict(
                "database is partially initialized or contains unrelated public objects",
            ))
        }
    }
}

fn validate_media_root(path: &Path) -> Result<(), BootstrapError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(BootstrapError::Conflict(
            "media root must be an existing non-symlink directory",
        ));
    }
    PaperclipRoot::open(path)?;
    if std::fs::read_dir(path)?.next().transpose()?.is_some() {
        return Err(BootstrapError::Conflict("media root is not empty"));
    }
    Ok(())
}

async fn validate_installer(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), BootstrapError> {
    let valid = sqlx::query_scalar::<_, bool>(
        "SELECT current_setting('server_version_num')::integer / 10000 = 14 \
           AND current_user = session_user \
           AND database_record.datdba = (SELECT oid FROM pg_catalog.pg_roles WHERE rolname = current_user) \
           AND public_schema.nspowner = database_record.datdba \
         FROM pg_catalog.pg_database database_record \
         JOIN pg_catalog.pg_namespace public_schema ON public_schema.nspname = 'public' \
         WHERE database_record.datname = current_database()",
    )
    .fetch_optional(&mut **transaction)
    .await?;
    if valid == Some(true) {
        Ok(())
    } else {
        Err(BootstrapError::Conflict(
            "PostgreSQL 14 installer must connect directly as owner of the database and public schema",
        ))
    }
}

async fn validate_role(
    transaction: &mut Transaction<'_, Postgres>,
    role_name: &str,
    require_noinherit: bool,
) -> Result<(), BootstrapError> {
    let valid = sqlx::query_scalar::<_, bool>(
        "SELECT role.rolcanlogin \
           AND (role.rolvaliduntil IS NULL OR role.rolvaliduntil > clock_timestamp()) \
           AND NOT role.rolsuper AND NOT role.rolcreatedb AND NOT role.rolcreaterole \
           AND NOT role.rolreplication AND NOT role.rolbypassrls \
           AND (NOT $2 OR NOT role.rolinherit) \
           AND role.rolname <> current_user \
           AND role.oid <> database_record.datdba \
           AND role.rolconfig IS NULL \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_db_role_setting setting \
                            WHERE setting.setdatabase = database_record.oid \
                              AND setting.setrole IN (0, role.oid)) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_auth_members membership \
                            WHERE membership.member = role.oid OR membership.roleid = role.oid) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_database owned WHERE owned.datdba = role.oid) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_namespace owned WHERE owned.nspowner = role.oid) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_class owned WHERE owned.relowner = role.oid) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_proc owned WHERE owned.proowner = role.oid) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_type owned WHERE owned.typowner = role.oid) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_default_acl defaults \
                            CROSS JOIN LATERAL pg_catalog.aclexplode(defaults.defaclacl) acl \
                            WHERE acl.grantee = role.oid) \
           AND NOT EXISTS ( \
             SELECT 1 FROM pg_catalog.pg_database object \
             CROSS JOIN LATERAL pg_catalog.aclexplode( \
               COALESCE(object.datacl, pg_catalog.acldefault('d', object.datdba))) acl \
             WHERE acl.grantee = role.oid AND object.oid <> database_record.oid \
             UNION ALL \
             SELECT 1 FROM pg_catalog.pg_namespace object \
             CROSS JOIN LATERAL pg_catalog.aclexplode( \
               COALESCE(object.nspacl, pg_catalog.acldefault('n', object.nspowner))) acl \
             WHERE acl.grantee = role.oid AND object.nspname NOT IN ('public', 'rustodon') \
             UNION ALL \
             SELECT 1 FROM pg_catalog.pg_class object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.relnamespace \
             CROSS JOIN LATERAL pg_catalog.aclexplode(COALESCE( \
               object.relacl, pg_catalog.acldefault( \
                 CASE WHEN object.relkind = 'S' THEN 's'::\"char\" ELSE 'r'::\"char\" END, \
                 object.relowner))) acl \
             WHERE acl.grantee = role.oid AND namespace.nspname NOT IN ('public', 'rustodon') \
             UNION ALL \
             SELECT 1 FROM pg_catalog.pg_proc object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.pronamespace \
             CROSS JOIN LATERAL pg_catalog.aclexplode( \
               COALESCE(object.proacl, pg_catalog.acldefault('f', object.proowner))) acl \
             WHERE acl.grantee = role.oid AND namespace.nspname NOT IN ('public', 'rustodon')) \
         FROM pg_catalog.pg_roles role \
         JOIN pg_catalog.pg_database database_record ON database_record.datname = current_database() \
         WHERE role.rolname = $1",
    )
    .bind(role_name)
    .bind(require_noinherit)
    .fetch_optional(&mut **transaction)
    .await?;
    if valid == Some(true) {
        Ok(())
    } else {
        Err(BootstrapError::Conflict(if require_noinherit {
            "runtime role is missing or has unsafe attributes, ownership, settings, or memberships"
        } else {
            "writer role is missing or has unsafe attributes, ownership, settings, or memberships"
        }))
    }
}

async fn database_state(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<DatabaseState, BootstrapError> {
    let public_initialized = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
           SELECT 1 FROM pg_catalog.pg_class relation \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           WHERE namespace.nspname = 'public' \
           UNION ALL \
           SELECT 1 FROM pg_catalog.pg_proc function_record \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = function_record.pronamespace \
           WHERE namespace.nspname = 'public')",
    )
    .fetch_one(&mut **transaction)
    .await?;
    let rustodon_exists =
        sqlx::query_scalar::<_, bool>("SELECT pg_catalog.to_regnamespace('rustodon') IS NOT NULL")
            .fetch_one(&mut **transaction)
            .await?;
    Ok(classify_database_state(public_initialized, rustodon_exists))
}

async fn validate_database_envelope(
    transaction: &mut Transaction<'_, Postgres>,
    allow_rustodon_schema: bool,
) -> Result<(), BootstrapError> {
    let valid = sqlx::query_scalar::<_, bool>(
        "SELECT \
           NOT EXISTS (SELECT 1 FROM pg_catalog.pg_namespace namespace \
                       WHERE namespace.nspname <> 'public' \
                         AND namespace.nspname <> 'information_schema' \
                         AND namespace.nspname !~ '^pg_' \
                         AND (NOT $1 OR namespace.nspname <> 'rustodon')) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_extension WHERE extname <> 'plpgsql') \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_publication) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_subscription) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_largeobject_metadata) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_cast WHERE oid >= 16384) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_transform WHERE oid >= 16384) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_ts_parser WHERE oid >= 16384) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_ts_template WHERE oid >= 16384) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_language WHERE oid >= 16384) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_foreign_data_wrapper WHERE oid >= 16384) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_foreign_server WHERE oid >= 16384) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_user_mappings WHERE umid >= 16384) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_am WHERE oid >= 16384) \
           AND NOT EXISTS ( \
             SELECT 1 FROM pg_catalog.pg_trigger trigger \
             JOIN pg_catalog.pg_class relation ON relation.oid = trigger.tgrelid \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
             WHERE namespace.nspname IN ('public', 'rustodon') AND NOT trigger.tgisinternal \
             UNION ALL \
             SELECT 1 FROM pg_catalog.pg_rewrite rule \
             JOIN pg_catalog.pg_class relation ON relation.oid = rule.ev_class \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
             WHERE namespace.nspname IN ('public', 'rustodon') \
               AND NOT (relation.relkind IN ('v', 'm') AND rule.rulename = '_RETURN') \
             UNION ALL \
             SELECT 1 FROM pg_catalog.pg_policy policy \
             JOIN pg_catalog.pg_class relation ON relation.oid = policy.polrelid \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
             WHERE namespace.nspname IN ('public', 'rustodon')) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_default_acl) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_db_role_setting setting \
                           JOIN pg_catalog.pg_database database_record \
                             ON database_record.oid = setting.setdatabase \
                           WHERE database_record.datname = current_database() \
                             AND setting.setrole = 0)",
    )
    .bind(allow_rustodon_schema)
    .fetch_one(&mut **transaction)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(BootstrapError::Conflict(
            "database contains unsupported schemas, hooks, extensions, global objects, defaults, or settings",
        ))
    }
}

async fn validate_pristine_database(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), BootstrapError> {
    let pristine = sqlx::query_scalar::<_, bool>(
        "SELECT \
           NOT EXISTS (SELECT 1 FROM pg_catalog.pg_namespace namespace \
                       WHERE namespace.nspname <> 'public' \
                         AND namespace.nspname <> 'information_schema' \
                         AND namespace.nspname !~ '^pg_') \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_extension WHERE extname <> 'plpgsql') \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_publication) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_subscription) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_largeobject_metadata) \
           AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_default_acl) \
           AND NOT EXISTS ( \
             SELECT 1 FROM pg_catalog.pg_type object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.typnamespace \
             WHERE namespace.nspname = 'public' \
             UNION ALL SELECT 1 FROM pg_catalog.pg_collation object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.collnamespace \
             WHERE namespace.nspname = 'public' \
             UNION ALL SELECT 1 FROM pg_catalog.pg_conversion object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.connamespace \
             WHERE namespace.nspname = 'public' \
             UNION ALL SELECT 1 FROM pg_catalog.pg_operator object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.oprnamespace \
             WHERE namespace.nspname = 'public' \
             UNION ALL SELECT 1 FROM pg_catalog.pg_opclass object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.opcnamespace \
             WHERE namespace.nspname = 'public' \
             UNION ALL SELECT 1 FROM pg_catalog.pg_opfamily object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.opfnamespace \
             WHERE namespace.nspname = 'public' \
             UNION ALL SELECT 1 FROM pg_catalog.pg_ts_dict object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.dictnamespace \
             WHERE namespace.nspname = 'public' \
             UNION ALL SELECT 1 FROM pg_catalog.pg_ts_config object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.cfgnamespace \
             WHERE namespace.nspname = 'public' \
             UNION ALL SELECT 1 FROM pg_catalog.pg_statistic_ext object \
             JOIN pg_catalog.pg_namespace namespace ON namespace.oid = object.stxnamespace \
             WHERE namespace.nspname = 'public') \
           AND NOT EXISTS ( \
             SELECT 1 FROM pg_catalog.pg_database database_record \
             CROSS JOIN LATERAL pg_catalog.aclexplode( \
               COALESCE(database_record.datacl, pg_catalog.acldefault('d', database_record.datdba))) acl \
             WHERE database_record.datname = current_database() \
               AND (acl.grantee NOT IN (0, database_record.datdba) \
                    OR (acl.grantee = 0 AND (acl.is_grantable OR acl.privilege_type = 'CREATE')))) \
           AND NOT EXISTS ( \
             SELECT 1 FROM pg_catalog.pg_namespace namespace \
             CROSS JOIN LATERAL pg_catalog.aclexplode( \
               COALESCE(namespace.nspacl, pg_catalog.acldefault('n', namespace.nspowner))) acl \
             WHERE namespace.nspname = 'public' \
               AND (acl.grantee NOT IN (0, namespace.nspowner) \
                    OR (acl.grantee = 0 AND acl.is_grantable)))",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if pristine {
        Ok(())
    } else {
        Err(BootstrapError::Conflict(
            "fresh database contains custom objects, hooks, ACLs, defaults, or schemas",
        ))
    }
}

async fn install_fresh(
    transaction: &mut Transaction<'_, Postgres>,
    inputs: &BootstrapInputs,
    password: &str,
) -> Result<(), BootstrapError> {
    let schema_sql = prepared_schema_sql(&random_hex_16())?;
    let versions = migration_versions()?;
    let actor_keys = signing_keys()?;
    let admin_keys = signing_keys()?;
    let password_hash = hash(password, DEFAULT_COST)
        .map_err(|_| BootstrapError::Cryptographic("password hashing failed"))?;

    sqlx::raw_sql(&schema_sql)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "SELECT pg_catalog.set_config('search_path', 'pg_catalog, public, pg_temp', true), \
                pg_catalog.set_config('TimeZone', 'UTC', true)",
    )
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO public.schema_migrations (version) \
         SELECT version FROM unnest($1::text[]) AS required(version)",
    )
    .bind(&versions)
    .execute(&mut **transaction)
    .await?;
    seed_roles(transaction).await?;
    seed_instance_actor(transaction, &actor_keys).await?;
    let admin_user_id = seed_admin(transaction, inputs, &admin_keys, &password_hash).await?;
    seed_settings_and_username_blocks(
        transaction,
        &inputs.site_title,
        &actor_keys.1,
        &admin_keys.1,
    )
    .await?;

    sqlx::raw_sql(
        "REFRESH MATERIALIZED VIEW public.account_summaries; \
         REFRESH MATERIALIZED VIEW public.global_follow_recommendations; \
         REFRESH MATERIALIZED VIEW public.instances;",
    )
    .execute(&mut **transaction)
    .await?;
    sqlx::raw_sql(include_str!("../docs/mastodon-refresh-instances.sql"))
        .execute(&mut **transaction)
        .await?;
    sqlx::query("SELECT pg_catalog.set_config('rustodon.writer_role', $1, true)")
        .bind(&inputs.writer_role)
        .execute(&mut **transaction)
        .await?;
    operational_schema::migrate_transaction(transaction).await?;
    crate::activity::record_activation_in(transaction, admin_user_id, false).await?;
    apply_writer_grants(transaction, &inputs.writer_role).await?;
    apply_runtime_grants(transaction, &inputs.runtime_role).await?;
    validate_complete(transaction, inputs, password).await
}

fn random_hex_16() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut output = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(output, "{byte:02x}").expect("writing to String is infallible");
    }
    output
}

fn signing_keys() -> Result<(String, String), BootstrapError> {
    let private_key = RsaPrivateKey::new(&mut OsRng, 2048)
        .map_err(|_| BootstrapError::Cryptographic("signing key generation failed"))?;
    let public_key = RsaPublicKey::from(&private_key)
        .to_public_key_pem(LineEnding::LF)
        .map_err(|_| BootstrapError::Cryptographic("signing key serialization failed"))?;
    let private_key = private_key
        .to_pkcs1_pem(LineEnding::LF)
        .map_err(|_| BootstrapError::Cryptographic("signing key serialization failed"))?
        .to_string();
    Ok((private_key, public_key))
}

fn public_key_fingerprint(public_key: &str) -> Result<String, BootstrapError> {
    let public_key = RsaPublicKey::from_public_key_pem(public_key)
        .map_err(|_| BootstrapError::Cryptographic("stored public key is invalid"))?;
    let der = public_key
        .to_public_key_der()
        .map_err(|_| BootstrapError::Cryptographic("public key fingerprint failed"))?;
    Ok(format!("{:x}", Sha256::digest(der.as_ref())))
}

fn validate_keypair(private_key: &str, public_key: &str) -> Result<(), BootstrapError> {
    let private_key = RsaPrivateKey::from_pkcs1_pem(private_key)
        .map_err(|_| BootstrapError::Cryptographic("stored private key is invalid"))?;
    let public_key = RsaPublicKey::from_public_key_pem(public_key)
        .map_err(|_| BootstrapError::Cryptographic("stored public key is invalid"))?;
    if private_key.n().bits() < 2048 || RsaPublicKey::from(&private_key) != public_key {
        return Err(BootstrapError::Conflict(
            "stored account signing keypair is invalid",
        ));
    }
    Ok(())
}

async fn seed_roles(transaction: &mut Transaction<'_, Postgres>) -> Result<(), BootstrapError> {
    sqlx::raw_sql(
        "INSERT INTO public.user_roles ( \
           id, collection_limit, color, created_at, highlighted, name, permissions, \
           position, require_2fa, updated_at) VALUES \
           (-99, 10, '', clock_timestamp(), false, '', 65536, -1, false, clock_timestamp()), \
           (1, 10, '', clock_timestamp(), true, 'Moderator', 1049884, 10, false, clock_timestamp()), \
           (2, 10, '', clock_timestamp(), true, 'Admin', 2031612, 100, false, clock_timestamp()), \
           (3, 10, '', clock_timestamp(), true, 'Owner', 1, 1000, false, clock_timestamp()); \
         SELECT pg_catalog.setval('public.user_roles_id_seq', 3, true);",
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn seed_instance_actor(
    transaction: &mut Transaction<'_, Postgres>,
    keys: &(String, String),
) -> Result<(), BootstrapError> {
    sqlx::query(
        "INSERT INTO public.accounts ( \
           id, username, actor_type, locked, private_key, public_key, created_at, updated_at) \
         VALUES (-99, 'mastodon.internal', 'Application', true, $1, $2, \
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(&keys.0)
    .bind(&keys.1)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO public.account_stats (account_id, created_at, updated_at) \
         VALUES (-99, clock_timestamp(), clock_timestamp())",
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn seed_admin(
    transaction: &mut Transaction<'_, Postgres>,
    inputs: &BootstrapInputs,
    keys: &(String, String),
    password_hash: &str,
) -> Result<i64, BootstrapError> {
    let user_id = sqlx::query_scalar(
        "WITH new_account AS ( \
           INSERT INTO public.accounts (username, actor_type, private_key, public_key, created_at, updated_at) \
           VALUES ($1, 'Person', $2, $3, clock_timestamp(), clock_timestamp()) RETURNING id), \
         new_stats AS ( \
           INSERT INTO public.account_stats (account_id, created_at, updated_at) \
           SELECT id, clock_timestamp(), clock_timestamp() FROM new_account RETURNING account_id) \
         INSERT INTO public.users ( \
           account_id, email, encrypted_password, approved, disabled, confirmed_at, role_id, \
           created_at, updated_at) \
         SELECT new_account.id, $4, $5, true, false, clock_timestamp(), 3, \
                clock_timestamp(), clock_timestamp() \
         FROM new_account JOIN new_stats ON new_stats.account_id = new_account.id RETURNING id",
    )
    .bind(&inputs.admin_username)
    .bind(&keys.0)
    .bind(&keys.1)
    .bind(&inputs.admin_email)
    .bind(password_hash)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(user_id)
}

async fn seed_settings_and_username_blocks(
    transaction: &mut Transaction<'_, Postgres>,
    site_title: &str,
    actor_public_key: &str,
    admin_public_key: &str,
) -> Result<(), BootstrapError> {
    let actor_fingerprint = public_key_fingerprint(actor_public_key)?;
    let admin_fingerprint = public_key_fingerprint(admin_public_key)?;
    sqlx::query(
        "INSERT INTO public.settings (var, value, created_at, updated_at) VALUES \
         ('site_title', $1, clock_timestamp(), clock_timestamp()), \
         ('rustodon_bootstrap_instance_key_sha256', $2, clock_timestamp(), clock_timestamp()), \
         ('rustodon_bootstrap_admin_key_sha256', $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(site_title_yaml(site_title))
    .bind(site_title_yaml(&actor_fingerprint))
    .bind(site_title_yaml(&admin_fingerprint))
    .execute(&mut **transaction)
    .await?;
    sqlx::raw_sql(
        "WITH blocked(username, exact) AS (VALUES \
           ('abuse', true), ('account', true), ('accounts', true), ('admin', true), \
           ('administration', true), ('administrator', true), ('admins', true), \
           ('help', true), ('helpdesk', true), ('instance', true), ('mod', true), \
           ('moderator', true), ('moderators', true), ('mods', true), ('owner', true), \
           ('root', true), ('security', true), ('server', true), ('staff', true), \
           ('support', true), ('webmaster', true), ('mastodon', false), ('mastadon', false)) \
         INSERT INTO public.username_blocks ( \
           username, normalized_username, exact, allow_with_approval, created_at, updated_at) \
         SELECT username, username, exact, false, clock_timestamp(), clock_timestamp() FROM blocked",
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn quoted_identifier(
    transaction: &mut Transaction<'_, Postgres>,
    value: &str,
) -> Result<String, BootstrapError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT pg_catalog.quote_ident($1)")
            .bind(value)
            .fetch_one(&mut **transaction)
            .await?,
    )
}

async fn apply_writer_grants(
    transaction: &mut Transaction<'_, Postgres>,
    writer_role: &str,
) -> Result<(), BootstrapError> {
    let quoted_role = quoted_identifier(transaction, writer_role).await?;
    let database = sqlx::query_scalar::<_, String>("SELECT pg_catalog.current_database()")
        .fetch_one(&mut **transaction)
        .await?;
    let quoted_database = quoted_identifier(transaction, &database).await?;
    let grants = prepared_writer_grants(&quoted_role, &quoted_database)?;
    sqlx::raw_sql(&grants).execute(&mut **transaction).await?;
    Ok(())
}

async fn apply_runtime_grants(
    transaction: &mut Transaction<'_, Postgres>,
    runtime_role: &str,
) -> Result<(), BootstrapError> {
    let role = quoted_identifier(transaction, runtime_role).await?;
    let database = sqlx::query_scalar::<_, String>("SELECT pg_catalog.current_database()")
        .fetch_one(&mut **transaction)
        .await?;
    let database = quoted_identifier(transaction, &database).await?;
    let sql = format!(
        "REVOKE ALL PRIVILEGES ON DATABASE {database} FROM {role}; \
         GRANT CONNECT ON DATABASE {database} TO {role}; \
         REVOKE ALL PRIVILEGES ON SCHEMA public FROM {role}; \
         GRANT USAGE ON SCHEMA public TO {role}; \
         REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA public FROM {role}; \
         GRANT SELECT ON ALL TABLES IN SCHEMA public TO {role}; \
         REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public FROM {role}; \
         GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO {role}; \
         REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA public FROM {role}; \
         REVOKE ALL PRIVILEGES ON SCHEMA rustodon FROM {role}; \
         GRANT USAGE ON SCHEMA rustodon TO {role}; \
         REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA rustodon FROM {role}; \
         REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA rustodon FROM {role}; \
         REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA rustodon FROM {role}; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE \
           rustodon.durable_jobs, rustodon.outbox_events, rustodon.idempotency_keys, \
           rustodon.ordering_markers, rustodon.domain_health, rustodon.heartbeats, \
           rustodon.rate_limit_windows TO {role}; \
         GRANT SELECT, INSERT, DELETE ON TABLE rustodon.remote_fetch_leases TO {role}; \
         GRANT SELECT ON TABLE rustodon.schema_migrations, \
           rustodon.activity_buckets, rustodon.activity_members TO {role}; \
         GRANT USAGE ON SEQUENCE rustodon.durable_jobs_id_seq, \
           rustodon.outbox_events_id_seq TO {role};"
    );
    sqlx::raw_sql(&sql).execute(&mut **transaction).await?;
    Ok(())
}

async fn validate_installed_acls(
    transaction: &mut Transaction<'_, Postgres>,
    runtime_role: &str,
    writer_role: &str,
) -> Result<(), BootstrapError> {
    let valid = sqlx::query_scalar::<_, bool>(
        "WITH roles AS ( \
           SELECT database_record.datdba AS owner_oid, runtime.oid AS runtime_oid, writer.oid AS writer_oid \
           FROM pg_catalog.pg_database database_record \
           JOIN pg_catalog.pg_roles runtime ON runtime.rolname = $1 \
           JOIN pg_catalog.pg_roles writer ON writer.rolname = $2 \
           WHERE database_record.datname = current_database()), \
         acl_entries AS ( \
           SELECT database_record.datname::text AS scope, acl.grantee, acl.is_grantable \
           FROM pg_catalog.pg_database database_record \
           CROSS JOIN LATERAL pg_catalog.aclexplode( \
             COALESCE(database_record.datacl, pg_catalog.acldefault('d', database_record.datdba))) acl \
           WHERE database_record.datname = current_database() \
           UNION ALL \
           SELECT namespace.nspname::text, acl.grantee, acl.is_grantable \
           FROM pg_catalog.pg_namespace namespace \
           CROSS JOIN LATERAL pg_catalog.aclexplode( \
             COALESCE(namespace.nspacl, pg_catalog.acldefault('n', namespace.nspowner))) acl \
           WHERE namespace.nspname IN ('public', 'rustodon') \
           UNION ALL \
           SELECT namespace.nspname::text, acl.grantee, acl.is_grantable \
           FROM pg_catalog.pg_class relation \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           CROSS JOIN LATERAL pg_catalog.aclexplode(COALESCE( \
             relation.relacl, pg_catalog.acldefault( \
               CASE WHEN relation.relkind = 'S' THEN 's'::\"char\" ELSE 'r'::\"char\" END, \
               relation.relowner))) acl \
           WHERE namespace.nspname IN ('public', 'rustodon') \
           UNION ALL \
           SELECT namespace.nspname::text, acl.grantee, acl.is_grantable \
           FROM pg_catalog.pg_attribute attribute \
           JOIN pg_catalog.pg_class relation ON relation.oid = attribute.attrelid \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           CROSS JOIN LATERAL pg_catalog.aclexplode(attribute.attacl) acl \
           WHERE namespace.nspname IN ('public', 'rustodon') \
           UNION ALL \
           SELECT namespace.nspname::text, acl.grantee, acl.is_grantable \
           FROM pg_catalog.pg_proc function_record \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = function_record.pronamespace \
           CROSS JOIN LATERAL pg_catalog.aclexplode( \
             COALESCE(function_record.proacl, pg_catalog.acldefault('f', function_record.proowner))) acl \
           WHERE namespace.nspname IN ('public', 'rustodon') \
           UNION ALL \
           SELECT namespace.nspname::text, acl.grantee, acl.is_grantable \
           FROM pg_catalog.pg_type type_record \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = type_record.typnamespace \
           CROSS JOIN LATERAL pg_catalog.aclexplode(type_record.typacl) acl \
           WHERE namespace.nspname IN ('public', 'rustodon') AND acl.grantee <> 0) \
         SELECT NOT EXISTS ( \
           SELECT 1 FROM acl_entries acl CROSS JOIN roles \
           WHERE acl.grantee = 0 \
              OR acl.grantee NOT IN (roles.owner_oid, roles.runtime_oid, roles.writer_oid) \
              OR (acl.is_grantable AND acl.grantee <> roles.owner_oid)) \
         AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_default_acl)",
    )
    .bind(runtime_role)
    .bind(writer_role)
    .fetch_one(&mut **transaction)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(BootstrapError::Conflict(
            "database contains unsupported recipients, PUBLIC grants, grant options, or default ACLs",
        ))
    }
}

async fn validate_empty_work_tables(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), BootstrapError> {
    let relations = sqlx::query_scalar::<_, String>(
        "SELECT pg_catalog.format('%I.%I', namespace.nspname, relation.relname) \
         FROM pg_catalog.pg_class relation \
         JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
         WHERE relation.relkind IN ('r', 'p') \
           AND namespace.nspname IN ('public', 'rustodon') \
           AND NOT (namespace.nspname = 'public' AND relation.relname IN ( \
             'accounts', 'account_stats', 'users', 'user_roles', 'username_blocks', \
             'settings', 'schema_migrations')) \
           AND NOT (namespace.nspname = 'rustodon' AND relation.relname IN ( \
             'schema_migrations', 'activity_buckets', 'activity_members')) \
         ORDER BY namespace.nspname COLLATE \"C\", relation.relname COLLATE \"C\"",
    )
    .fetch_all(&mut **transaction)
    .await?;
    let query = format!(
        "SELECT {}",
        relations
            .iter()
            .map(|relation| format!("NOT EXISTS (SELECT 1 FROM {relation} LIMIT 1)"))
            .collect::<Vec<_>>()
            .join(" AND ")
    );
    if !sqlx::query_scalar::<_, bool>(&query)
        .fetch_one(&mut **transaction)
        .await?
    {
        return Err(BootstrapError::Conflict(
            "standalone baseline contains unexpected Mastodon or operational rows",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn validate_complete(
    transaction: &mut Transaction<'_, Postgres>,
    inputs: &BootstrapInputs,
    password: &str,
) -> Result<(), BootstrapError> {
    let versions = migration_versions()?;
    let baseline_valid = sqlx::query_scalar::<_, bool>(
        "SELECT \
           (SELECT array_agg(version::text ORDER BY version COLLATE \"C\") FROM public.schema_migrations) \
             = (SELECT array_agg(version ORDER BY version COLLATE \"C\") FROM unnest($1::text[]) version) \
           AND (SELECT count(*) FROM public.accounts) = 2 \
           AND (SELECT count(*) FROM public.users) = 1 \
           AND (SELECT count(*) FROM rustodon.activity_members) = 1 \
           AND (SELECT count(*) FROM rustodon.activity_buckets) = 1 \
           AND EXISTS (SELECT 1 FROM rustodon.activity_members member \
             JOIN rustodon.activity_buckets bucket USING (day) \
             JOIN public.users user_record ON user_record.id = member.user_id \
             WHERE bucket.day BETWEEN user_record.created_at::date \
               AND ((bucket.expires_at - make_interval(secs => $5::bigint)) AT TIME ZONE 'UTC')::date \
               AND bucket.expires_at >= (user_record.created_at AT TIME ZONE 'UTC') + make_interval(secs => $5::bigint) \
               AND bucket.expires_at <= clock_timestamp() + make_interval(secs => $5::bigint)) \
           AND (SELECT count(*) FROM public.account_stats) = 2 \
           AND (SELECT count(*) FROM public.user_roles) = 4 \
           AND (SELECT count(*) FROM public.username_blocks) = 23 \
           AND (SELECT count(*) FROM public.settings) = 3 \
           AND (SELECT count(*) FROM public.statuses) = 0 \
           AND (SELECT count(*) FROM public.media_attachments) = 0 \
           AND (SELECT count(*) FROM public.oauth_access_tokens) = 0 \
           AND (SELECT count(*) FROM public.session_activations) = 0 \
           AND (SELECT count(*) FROM public.keypairs) = 0 \
           AND EXISTS (SELECT 1 FROM public.accounts actor \
                       JOIN public.account_stats stats ON stats.account_id = actor.id \
                       WHERE actor.id = -99 AND actor.username = 'mastodon.internal' \
                         AND actor.domain IS NULL AND actor.actor_type = 'Application' \
                         AND actor.locked AND NOT actor.memorial AND actor.suspended_at IS NULL \
                         AND actor.moved_to_account_id IS NULL \
                         AND actor.private_key <> '' AND actor.public_key <> '' \
                         AND stats.followers_count = 0 AND stats.following_count = 0 \
                         AND stats.statuses_count = 0 AND stats.last_status_at IS NULL) \
           AND EXISTS (SELECT 1 FROM public.users user_record \
                       JOIN public.accounts account ON account.id = user_record.account_id \
                       JOIN public.account_stats stats ON stats.account_id = account.id \
                       WHERE account.username = $2 AND account.domain IS NULL \
                         AND account.actor_type = 'Person' AND NOT account.locked \
                         AND NOT account.memorial AND account.suspended_at IS NULL \
                         AND account.moved_to_account_id IS NULL \
                         AND account.private_key <> '' AND account.public_key <> '' \
                         AND stats.followers_count = 0 AND stats.following_count = 0 \
                         AND stats.statuses_count = 0 AND stats.last_status_at IS NULL \
                         AND user_record.email = $3 AND user_record.role_id = 3 \
                         AND user_record.approved AND NOT user_record.disabled \
                         AND user_record.confirmed_at IS NOT NULL \
                         AND NOT user_record.otp_required_for_login \
                         AND user_record.sign_in_count = 0) \
           AND EXISTS (SELECT 1 FROM public.settings \
                       WHERE var = 'site_title' AND value = $4) \
           AND NOT EXISTS ((VALUES \
             (-99::bigint, 10, ''::text, false, ''::text, 65536::bigint, -1, false), \
             (1, 10, '', true, 'Moderator', 1049884, 10, false), \
             (2, 10, '', true, 'Admin', 2031612, 100, false), \
             (3, 10, '', true, 'Owner', 1, 1000, false)) \
             EXCEPT SELECT id, collection_limit, color::text, highlighted, name::text, \
                           permissions, position, require_2fa FROM public.user_roles) \
           AND NOT EXISTS ((VALUES \
             ('abuse'::text, true), ('account', true), ('accounts', true), ('admin', true), \
             ('administration', true), ('administrator', true), ('admins', true), \
             ('help', true), ('helpdesk', true), ('instance', true), ('mod', true), \
             ('moderator', true), ('moderators', true), ('mods', true), ('owner', true), \
             ('root', true), ('security', true), ('server', true), ('staff', true), \
             ('support', true), ('webmaster', true), ('mastodon', false), ('mastadon', false)) \
             EXCEPT SELECT username::text, exact FROM public.username_blocks \
                    WHERE normalized_username = username AND NOT allow_with_approval)",
    )
    .bind(&versions)
    .bind(&inputs.admin_username)
    .bind(&inputs.admin_email)
    .bind(site_title_yaml(&inputs.site_title))
    .bind(crate::activity::RETENTION_SECONDS)
    .fetch_one(&mut **transaction)
    .await?;
    if !baseline_valid {
        return Err(BootstrapError::Conflict(
            "standalone baseline identities or empty-state data have drifted",
        ));
    }
    validate_empty_work_tables(transaction).await?;
    let keys = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, private_key, public_key FROM public.accounts ORDER BY id",
    )
    .fetch_all(&mut **transaction)
    .await?;
    if keys.len() != 2 || keys[0].0 != -99 || keys[0].2 == keys[1].2 {
        return Err(BootstrapError::Conflict(
            "standalone account signing identities have drifted",
        ));
    }
    validate_keypair(&keys[0].1, &keys[0].2)?;
    validate_keypair(&keys[1].1, &keys[1].2)?;
    let fingerprints = sqlx::query_as::<_, (String, String)>(
        "SELECT var, value FROM public.settings \
         WHERE var IN ('rustodon_bootstrap_instance_key_sha256', \
                       'rustodon_bootstrap_admin_key_sha256') \
         ORDER BY var COLLATE \"C\"",
    )
    .fetch_all(&mut **transaction)
    .await?;
    let expected_admin = site_title_yaml(&public_key_fingerprint(&keys[1].2)?);
    let expected_instance = site_title_yaml(&public_key_fingerprint(&keys[0].2)?);
    if fingerprints
        != [
            (
                "rustodon_bootstrap_admin_key_sha256".to_owned(),
                expected_admin,
            ),
            (
                "rustodon_bootstrap_instance_key_sha256".to_owned(),
                expected_instance,
            ),
        ]
    {
        return Err(BootstrapError::Conflict(
            "standalone signing-key fingerprints have drifted",
        ));
    }
    let password_hash =
        sqlx::query_scalar::<_, String>("SELECT encrypted_password FROM public.users LIMIT 1")
            .fetch_one(&mut **transaction)
            .await?;
    if !verify(password, &password_hash)
        .map_err(|_| BootstrapError::Cryptographic("stored password hash is invalid"))?
    {
        return Err(BootstrapError::Conflict(
            "admin password does not match the completed bootstrap",
        ));
    }
    operational_schema::validate_bootstrap_transaction(
        transaction,
        &inputs.runtime_role,
        &inputs.writer_role,
    )
    .await?;
    validate_installed_acls(transaction, &inputs.runtime_role, &inputs.writer_role).await?;
    preflight::validate_named_writer_role_in_transaction(transaction, &inputs.writer_role)
        .await
        .map_err(BootstrapError::Validation)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use saphyr::LoadableYamlNode;

    use super::{
        DatabaseState, classify_database_state, migration_versions, prepared_schema_sql,
        prepared_writer_grants, site_title_yaml, validate_inputs,
    };

    #[test]
    fn schema_salt_replacement_is_exact_and_fail_closed() {
        let salt = "0123456789abcdef0123456789abcdef";
        let sql = prepared_schema_sql(salt).expect("valid salt and pinned artifact");
        assert!(!sql.contains("__RUSTODON_TIMESTAMP_ID_SALT__"));
        assert!(!sql.contains("SET statement_timeout = 0;"));
        assert!(!sql.contains("SET lock_timeout = 0;"));
        assert!(sql.contains("SET LOCAL statement_timeout TO '5min';"));
        assert!(sql.contains("SET LOCAL lock_timeout TO '10s';"));
        assert!(sql.contains(salt));
        assert!(prepared_schema_sql("NOT-LOWERCASE-HEX").is_err());
    }

    #[test]
    fn database_state_only_accepts_pristine_or_complete_shapes() {
        assert_eq!(classify_database_state(false, false), DatabaseState::Fresh);
        assert_eq!(classify_database_state(true, true), DatabaseState::Complete);
        assert_eq!(
            classify_database_state(true, false),
            DatabaseState::Conflicting
        );
        assert_eq!(
            classify_database_state(false, true),
            DatabaseState::Conflicting
        );
    }

    #[test]
    fn bootstrap_inputs_normalize_and_reject_reserved_admin_names() {
        let inputs = validate_inputs(
            " Alice_1 ",
            " OWNER@Example.COM ",
            "correct horse battery staple",
            " Rustodon ",
            "rustodon_runtime",
            "rustodon_writer",
            Path::new("/srv/rustodon/system"),
        )
        .expect("valid bootstrap inputs");
        assert_eq!(inputs.admin_username, "alice_1");
        assert_eq!(inputs.admin_email, "owner@example.com");
        assert_eq!(inputs.site_title, "Rustodon");

        assert!(
            validate_inputs(
                "admin",
                "owner@example.com",
                "correct horse battery staple",
                "Rustodon",
                "rustodon_runtime",
                "rustodon_writer",
                Path::new("/srv/rustodon/system"),
            )
            .is_err()
        );
        assert!(
            validate_inputs(
                "my_mastodon_admin",
                "owner@example.com",
                "correct horse battery staple",
                "Rustodon",
                "rustodon_runtime",
                "rustodon_writer",
                Path::new("/srv/rustodon/system"),
            )
            .is_err()
        );
    }

    #[test]
    fn bootstrap_inputs_require_distinct_roles_and_absolute_media_root() {
        assert!(
            validate_inputs(
                "alice",
                "owner@example.com",
                "correct horse battery staple",
                "Rustodon",
                "rustodon",
                "rustodon",
                Path::new("/srv/rustodon/system"),
            )
            .is_err()
        );
        assert!(
            validate_inputs(
                "alice",
                "owner@example.com",
                "correct horse battery staple",
                "Rustodon",
                "rustodon_runtime",
                "rustodon_writer",
                Path::new("relative/system"),
            )
            .is_err()
        );
    }

    #[test]
    fn migration_inventory_is_the_exact_pinned_set() {
        let versions = migration_versions().expect("valid pinned migration inventory");
        assert_eq!(versions.len(), 588);
        assert_eq!(versions.first().map(String::as_str), Some("20160220174730"));
        assert_eq!(versions.last().map(String::as_str), Some("20260611150940"));
    }

    #[test]
    fn site_title_is_encoded_as_a_yaml_string() {
        assert_eq!(site_title_yaml("Alice's place"), "--- 'Alice''s place'\n");
        let parsed = saphyr::Yaml::load_from_str(&site_title_yaml("Alice's place"))
            .expect("generated title YAML parses");
        assert_eq!(parsed[0].as_str(), Some("Alice's place"));
    }

    #[test]
    fn writer_grants_are_sqlx_compatible_and_safely_substituted() {
        let sql = prepared_writer_grants("\"writer role\"", "\"database-name\"")
            .expect("pinned writer script transforms");
        assert!(!sql.contains("\\set"));
        assert!(!sql.contains("\\gexec"));
        assert!(!sql.contains(":\"writer_role\""));
        assert!(!sql.contains("BEGIN;"));
        assert!(!sql.contains("COMMIT;"));
        assert!(sql.contains("GRANT CONNECT ON DATABASE \"database-name\" TO \"writer role\""));
        assert!(
            sql.contains("GRANT EXECUTE ON FUNCTION public.timestamp_id(text) TO \"writer role\"")
        );
    }
}
