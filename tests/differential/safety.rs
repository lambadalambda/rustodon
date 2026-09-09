use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use sqlx::{Connection, PgConnection};
use url::Url;

const RUN_ID_ENV: &str = "RUSTODON_DIFFERENTIAL_RUN_ID";
const MASTODON_HTTP_ENV: &str = "RUSTODON_DIFFERENTIAL_MASTODON_HTTP_URL";
const MASTODON_DATABASE_ENV: &str = "RUSTODON_DIFFERENTIAL_MASTODON_DATABASE_URL";
const MASTODON_OWNER_DATABASE_ENV: &str = "RUSTODON_DIFFERENTIAL_MASTODON_OWNER_DATABASE_URL";
const RUST_DATABASE_ENV: &str = "RUSTODON_DIFFERENTIAL_RUST_DATABASE_URL";
const RUST_WRITE_DATABASE_ENV: &str = "RUSTODON_DIFFERENTIAL_RUST_WRITE_DATABASE_URL";
const RUST_OWNER_DATABASE_ENV: &str = "RUSTODON_DIFFERENTIAL_RUST_OWNER_DATABASE_URL";
const MEDIA_MARKER: &str = ".rustodon-differential-media-root";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Mastodon,
    Rust,
}

impl Side {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Mastodon => "mastodon",
            Self::Rust => "rust",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HttpTargets {
    mastodon: Url,
    rust: Url,
}

impl HttpTargets {
    pub(crate) fn new(mastodon: &str, rust: &str) -> Result<Self, SafetyError> {
        let mastodon = validate_http_endpoint(mastodon)?;
        let rust = validate_http_endpoint(rust)?;
        if mastodon == rust {
            return Err(SafetyError("HTTP targets must be distinct".to_owned()));
        }
        Ok(Self { mastodon, rust })
    }

    pub(crate) const fn mastodon(&self) -> &Url {
        &self.mastodon
    }

    pub(crate) const fn rust(&self) -> &Url {
        &self.rust
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DatabaseTarget {
    url: Url,
    name: String,
    side: Side,
}

impl DatabaseTarget {
    pub(crate) fn url(&self) -> &str {
        self.url.as_str()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DifferentialConfig {
    pub(crate) run_id: String,
    pub(crate) mastodon_http: Url,
    pub(crate) mastodon_database: DatabaseTarget,
    pub(crate) mastodon_owner_database: Option<DatabaseTarget>,
    pub(crate) rust_database: DatabaseTarget,
    pub(crate) rust_write_database: Option<DatabaseTarget>,
    pub(crate) rust_owner_database: Option<DatabaseTarget>,
    pub(crate) mastodon_media: PathBuf,
    pub(crate) rust_media: PathBuf,
}

impl DifferentialConfig {
    pub(crate) fn from_process_environment(repository: &Path) -> Result<Self, SafetyError> {
        let environment = std::env::vars().collect::<HashMap<_, _>>();
        Self::from_environment(&environment, repository)
    }

    pub(crate) fn from_environment(
        environment: &HashMap<String, String>,
        repository: &Path,
    ) -> Result<Self, SafetyError> {
        let run_id = required(environment, RUN_ID_ENV)?;
        validate_run_id(run_id)?;
        let mastodon_http = validate_http_endpoint(required(environment, MASTODON_HTTP_ENV)?)?;
        let mastodon_database = validate_database_url(
            required(environment, MASTODON_DATABASE_ENV)?,
            run_id,
            Side::Mastodon,
        )?;
        let mastodon_owner_database = optional_database_url(
            environment,
            MASTODON_OWNER_DATABASE_ENV,
            run_id,
            Side::Mastodon,
        )?;
        let rust_database = validate_database_url(
            required(environment, RUST_DATABASE_ENV)?,
            run_id,
            Side::Rust,
        )?;
        let rust_write_database =
            optional_database_url(environment, RUST_WRITE_DATABASE_ENV, run_id, Side::Rust)?;
        let rust_owner_database =
            optional_database_url(environment, RUST_OWNER_DATABASE_ENV, run_id, Side::Rust)?;
        if mastodon_owner_database.is_some() != rust_write_database.is_some() {
            return Err(SafetyError(
                "differential writer targets must be provided together".to_owned(),
            ));
        }
        if mastodon_database.url == rust_database.url
            || mastodon_database.name == rust_database.name
        {
            return Err(SafetyError("database targets must be distinct".to_owned()));
        }
        if let Some(owner) = &mastodon_owner_database
            && (owner.url == mastodon_database.url || owner.name != mastodon_database.name)
        {
            return Err(SafetyError(
                "Mastodon writer target must use the marked Mastodon database with distinct credentials"
                    .to_owned(),
            ));
        }
        if let Some(writer) = &rust_write_database
            && (writer.url == rust_database.url || writer.name != rust_database.name)
        {
            return Err(SafetyError(
                "Rust writer target must use the marked Rust database with distinct credentials"
                    .to_owned(),
            ));
        }
        if let Some(owner) = &rust_owner_database
            && (owner.url == rust_database.url || owner.name != rust_database.name)
        {
            return Err(SafetyError(
                "Rust owner target must use the marked Rust database with distinct credentials"
                    .to_owned(),
            ));
        }
        let (mastodon_media, rust_media) = validate_media_roots(repository, run_id)?;
        Ok(Self {
            run_id: run_id.to_owned(),
            mastodon_http,
            mastodon_database,
            mastodon_owner_database,
            rust_database,
            rust_write_database,
            rust_owner_database,
            mastodon_media,
            rust_media,
        })
    }

    pub(crate) async fn validate_database_comments(&self) -> Result<(), SafetyError> {
        let mut targets = vec![&self.mastodon_database, &self.rust_database];
        if let Some(target) = &self.mastodon_owner_database {
            targets.push(target);
        }
        if let Some(target) = &self.rust_write_database {
            targets.push(target);
        }
        if let Some(target) = &self.rust_owner_database {
            targets.push(target);
        }
        for target in targets {
            let mut connection = PgConnection::connect(target.url()).await.map_err(|error| {
                SafetyError(format!("database marker connection failed: {error}"))
            })?;
            let (name, comment): (String, Option<String>) = sqlx::query_as(
                "SELECT current_database(), pg_catalog.shobj_description(oid, 'pg_database') \
                 FROM pg_catalog.pg_database WHERE datname = current_database()",
            )
            .fetch_one(&mut connection)
            .await
            .map_err(|error| SafetyError(format!("database marker query failed: {error}")))?;
            let expected = database_comment(&self.run_id, target.side);
            if name != target.name || comment.as_deref() != Some(&expected) {
                return Err(SafetyError(format!(
                    "database marker mismatch for {}: expected database {} comment {:?}, got {} {:?}",
                    target.side.name(),
                    target.name,
                    expected,
                    name,
                    comment
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SafetyError(pub(crate) String);

impl fmt::Display for SafetyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SafetyError {}

pub(crate) fn validate_http_endpoint(input: &str) -> Result<Url, SafetyError> {
    let authority = input
        .strip_prefix("http://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .ok_or_else(|| {
            SafetyError("HTTP target must use http://127.0.0.1 with an explicit port".to_owned())
        })?;
    let Some((host, port)) = authority.rsplit_once(':') else {
        return Err(SafetyError(
            "HTTP target must have an explicit port".to_owned(),
        ));
    };
    if host != "127.0.0.1" || !port.parse::<u16>().is_ok_and(|port| port > 0) {
        return Err(SafetyError(
            "HTTP target must be http://127.0.0.1 with an explicit nonzero port".to_owned(),
        ));
    }

    let url = Url::parse(input)
        .map_err(|error| SafetyError(format!("invalid HTTP target URL: {error}")))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(SafetyError(
            "HTTP target must be an origin-only http://127.0.0.1:<port> URL".to_owned(),
        ));
    }
    Ok(url)
}

fn validate_database_url(
    input: &str,
    run_id: &str,
    side: Side,
) -> Result<DatabaseTarget, SafetyError> {
    let url =
        Url::parse(input).map_err(|error| SafetyError(format!("invalid database URL: {error}")))?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err(SafetyError(
            "database URL must use postgres:// or postgresql://".to_owned(),
        ));
    }
    let host = url
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .filter(IpAddr::is_loopback)
        .ok_or_else(|| SafetyError("database URL host must be a numeric loopback IP".to_owned()))?;
    if !host.is_loopback() || url.query().is_some() || url.fragment().is_some() {
        return Err(SafetyError(
            "database URL must identify one loopback database without query or fragment".to_owned(),
        ));
    }
    let name = url.path().strip_prefix('/').unwrap_or_default().to_owned();
    let expected_name = database_name(run_id, side);
    if name != expected_name || name.contains('/') {
        return Err(SafetyError(format!(
            "database name must be exactly {expected_name}"
        )));
    }
    Ok(DatabaseTarget { url, name, side })
}

fn optional_database_url(
    environment: &HashMap<String, String>,
    variable: &str,
    run_id: &str,
    side: Side,
) -> Result<Option<DatabaseTarget>, SafetyError> {
    environment
        .get(variable)
        .map(|value| validate_database_url(value, run_id, side))
        .transpose()
}

fn validate_media_roots(
    repository: &Path,
    run_id: &str,
) -> Result<(PathBuf, PathBuf), SafetyError> {
    let target = fs::canonicalize(repository.join("target"))
        .map_err(|error| SafetyError(format!("cannot canonicalize target directory: {error}")))?;
    let run_root_input = repository
        .join("target")
        .join(format!("differential-{run_id}"));
    reject_symlink(&run_root_input)?;
    let run_root = fs::canonicalize(&run_root_input).map_err(|error| {
        SafetyError(format!(
            "cannot canonicalize differential run root: {error}"
        ))
    })?;
    if run_root.parent() != Some(target.as_path()) {
        return Err(SafetyError(
            "differential run root must be a canonical direct child of target".to_owned(),
        ));
    }

    let mastodon = validate_media_root(&run_root, run_id, Side::Mastodon)?;
    let rust = validate_media_root(&run_root, run_id, Side::Rust)?;
    if mastodon == rust {
        return Err(SafetyError("media roots must be distinct".to_owned()));
    }
    Ok((mastodon, rust))
}

fn validate_media_root(run_root: &Path, run_id: &str, side: Side) -> Result<PathBuf, SafetyError> {
    let input = run_root.join(format!("{}-media", side.name()));
    reject_symlink(&input)?;
    let root = fs::canonicalize(&input).map_err(|error| {
        SafetyError(format!(
            "cannot canonicalize {} media root: {error}",
            side.name()
        ))
    })?;
    if root.parent() != Some(run_root) {
        return Err(SafetyError(format!(
            "{} media root must be a canonical direct child of the run root",
            side.name()
        )));
    }
    reject_tree_symlinks(&root)?;
    let marker = root.join(MEDIA_MARKER);
    reject_symlink(&marker)?;
    let contents = fs::read_to_string(&marker)
        .map_err(|error| SafetyError(format!("cannot read media marker: {error}")))?;
    let expected = media_marker(run_id, side);
    if contents != expected {
        return Err(SafetyError(format!(
            "{} media marker must exactly match {:?}",
            side.name(),
            expected
        )));
    }
    Ok(root)
}

fn reject_tree_symlinks(path: &Path) -> Result<(), SafetyError> {
    for entry in fs::read_dir(path)
        .map_err(|error| SafetyError(format!("cannot read media tree: {error}")))?
    {
        let entry =
            entry.map_err(|error| SafetyError(format!("cannot read media entry: {error}")))?;
        let file_type = entry
            .file_type()
            .map_err(|error| SafetyError(format!("cannot inspect media entry: {error}")))?;
        if file_type.is_symlink() {
            return Err(SafetyError(format!(
                "media tree contains symlink: {}",
                entry.path().display()
            )));
        }
        if file_type.is_dir() {
            reject_tree_symlinks(&entry.path())?;
        } else if !file_type.is_file() {
            return Err(SafetyError(format!(
                "media tree contains a non-file entry: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), SafetyError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| SafetyError(format!("cannot inspect {}: {error}", path.display())))?;
    if metadata.file_type().is_symlink() {
        Err(SafetyError(format!(
            "path must not be a symlink: {}",
            path.display()
        )))
    } else {
        Ok(())
    }
}

fn required<'a>(
    environment: &'a HashMap<String, String>,
    name: &str,
) -> Result<&'a str, SafetyError> {
    environment
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SafetyError(format!("required environment variable {name} is missing")))
}

fn validate_run_id(run_id: &str) -> Result<(), SafetyError> {
    if run_id.is_empty()
        || run_id.len() > 32
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(SafetyError(
            "run ID must be 1-32 lowercase ASCII letters, digits, or underscores".to_owned(),
        ));
    }
    Ok(())
}

fn database_name(run_id: &str, side: Side) -> String {
    format!("rustodon_differential_{run_id}_{}", side.name())
}

fn database_comment(run_id: &str, side: Side) -> String {
    format!("rustodon differential run={run_id} side={}", side.name())
}

fn media_marker(run_id: &str, side: Side) -> String {
    format!("run={run_id}\nside={}\n", side.name())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn run_id() -> String {
        format!(
            "unit_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should follow Unix epoch")
                .as_nanos()
        )
    }

    fn repository() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn environment(run_id: &str) -> HashMap<String, String> {
        HashMap::from([
            (RUN_ID_ENV.to_owned(), run_id.to_owned()),
            (
                MASTODON_HTTP_ENV.to_owned(),
                "http://127.0.0.1:31001".to_owned(),
            ),
            (
                MASTODON_DATABASE_ENV.to_owned(),
                format!(
                    "postgresql://fixture:secret@127.0.0.1:5432/{}",
                    database_name(run_id, Side::Mastodon)
                ),
            ),
            (
                RUST_DATABASE_ENV.to_owned(),
                format!(
                    "postgresql://fixture:secret@127.0.0.1:5432/{}",
                    database_name(run_id, Side::Rust)
                ),
            ),
        ])
    }

    fn create_media_roots(run_id: &str) -> PathBuf {
        let root = repository()
            .join("target")
            .join(format!("differential-{run_id}"));
        for side in [Side::Mastodon, Side::Rust] {
            let media = root.join(format!("{}-media", side.name()));
            fs::create_dir_all(&media).expect("test media root should be created");
            fs::write(media.join(MEDIA_MARKER), media_marker(run_id, side))
                .expect("test marker should be written");
        }
        root
    }

    #[test]
    fn http_targets_require_distinct_explicit_port_ipv4_loopback_origins() {
        assert!(HttpTargets::new("http://127.0.0.1:3001", "http://127.0.0.1:3002").is_ok());
        for invalid in [
            "https://127.0.0.1:3001",
            "http://localhost:3001",
            "http://127.0.0.1",
            "http://127.0.0.1:3001/path",
            "http://user@127.0.0.1:3001",
        ] {
            assert!(
                HttpTargets::new(invalid, "http://127.0.0.1:3002").is_err(),
                "accepted unsafe endpoint {invalid}"
            );
        }
        assert!(HttpTargets::new("http://127.0.0.1:3001", "http://127.0.0.1:3001").is_err());
    }

    #[test]
    fn database_urls_require_loopback_and_exact_run_scoped_names() {
        let run_id = "unit_database";
        let expected = format!(
            "postgresql://fixture@127.0.0.1:5432/{}",
            database_name(run_id, Side::Mastodon)
        );
        assert!(validate_database_url(&expected, run_id, Side::Mastodon).is_ok());
        for invalid in [
            "postgresql://fixture@db.example/rustodon_differential_unit_database_mastodon",
            "postgresql://fixture@127.0.0.1/production",
            "sqlite:///rustodon_differential_unit_database_mastodon",
        ] {
            assert!(validate_database_url(invalid, run_id, Side::Mastodon).is_err());
        }
    }

    #[test]
    fn configuration_uses_derived_canonical_media_roots_and_exact_markers() {
        let run_id = run_id();
        let root = create_media_roots(&run_id);
        let config = DifferentialConfig::from_environment(
            &environment(&run_id),
            Path::new(env!("CARGO_MANIFEST_DIR")),
        )
        .expect("guarded configuration should validate");
        assert_ne!(config.mastodon_media, config.rust_media);
        assert_eq!(config.mastodon_media.parent(), config.rust_media.parent());

        fs::write(
            config.rust_media.join(MEDIA_MARKER),
            "run=wrong\nside=rust\n",
        )
        .expect("marker should be replaceable");
        let error = DifferentialConfig::from_environment(&environment(&run_id), &repository())
            .expect_err("wrong marker must fail");
        assert!(error.to_string().contains("marker"));
        fs::remove_dir_all(root).expect("test run root should be removable");
    }

    #[test]
    fn map_based_validation_does_not_need_process_global_environment_mutation() {
        let run_id = run_id();
        let root = create_media_roots(&run_id);
        let mut values = environment(&run_id);
        values.remove(MASTODON_HTTP_ENV);
        let error = DifferentialConfig::from_environment(&values, &repository())
            .expect_err("missing map entry must fail");
        assert!(error.to_string().contains(MASTODON_HTTP_ENV));
        fs::remove_dir_all(root).expect("test run root should be removable");
    }
}
