use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use ipnetwork::IpNetwork;
use url::{Host, Url};
use zeroize::Zeroize;

use crate::secret::SecretString;

const REPLICA_VARIABLES: &[&str] = &[
    "REPLICA_DATABASE_URL",
    "READ_REPLICA_DATABASE_URL",
    "READ_REPLICA_URL",
    "REPLICA_DB_HOST",
    "REPLICA_DB_PORT",
    "REPLICA_DB_NAME",
    "REPLICA_DB_USER",
    "REPLICA_DB_PASS",
    "REPLICA_DB_SSLMODE",
    "REPLICA_DB_POOL",
    "REPLICA_PREPARED_STATEMENTS",
    "REPLICA_DB_TASKS",
];

const TRUSTED_PROXY_DEFAULTS: &[&str] = &[
    "127.0.0.1/8",
    "::1/128",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "fe80::/10",
    "fc00::/7",
];

const REDIS_SENTINEL_VARIABLES: &[&str] = &[
    "SIDEKIQ_REDIS_SENTINEL_MASTER",
    "SIDEKIQ_REDIS_SENTINEL_PORT",
    "SIDEKIQ_REDIS_SENTINELS",
    "SIDEKIQ_REDIS_SENTINEL_USERNAME",
    "SIDEKIQ_REDIS_SENTINEL_PASSWORD",
    "REDIS_SENTINEL_MASTER",
    "REDIS_SENTINEL_PORT",
    "REDIS_SENTINELS",
    "REDIS_SENTINEL_USERNAME",
    "REDIS_SENTINEL_PASSWORD",
];

/// Validated, typed configuration needed by Rustodon's v1 cutover.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub domains: DomainConfig,
    pub database: PostgresConfig,
    pub paperclip: PaperclipConfig,
    pub trusted_proxies: Vec<IpNetwork>,
    pub smtp: SmtpConfig,
    pub secrets: CryptographicSecrets,
    pub unsupported: UnsupportedConfiguration,
    pub sidekiq_redis: Option<RedisEndpoint>,
}

impl Config {
    /// Loads configuration from a caller-provided environment map.
    ///
    /// # Errors
    ///
    /// Returns an error naming the invalid or missing environment variable. Supplied secret
    /// values are never retained in the error.
    pub fn from_environment(environment: &HashMap<String, String>) -> Result<Self, ConfigError> {
        reject_replica_configuration(environment)?;

        let domains = parse_domains(environment)?;
        let database = parse_database(environment)?;
        let paperclip = parse_paperclip(environment)?;
        let trusted_proxies = parse_trusted_proxies(environment)?;
        let smtp = parse_smtp(environment, &domains)?;
        let secrets = parse_secrets(environment)?;
        let unsupported = parse_unsupported_configuration(environment)?;
        let sidekiq_redis = parse_sidekiq_redis(environment)?;

        Ok(Self {
            domains,
            database,
            paperclip,
            trusted_proxies,
            smtp,
            secrets,
            unsupported,
            sidekiq_redis,
        })
    }

    /// Loads configuration from the current process environment.
    ///
    /// # Errors
    ///
    /// Returns the same validation errors as [`Self::from_environment`].
    pub fn from_process_environment() -> Result<Self, ConfigError> {
        let mut environment = env::vars().collect::<HashMap<_, _>>();
        let config = Self::from_environment(&environment);
        for value in environment.values_mut() {
            value.zeroize();
        }
        config
    }
}

impl fmt::Display for Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("validated Rustodon configuration (secrets [REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainConfig {
    pub local_domain: String,
    pub web_domain: String,
    pub alternate_domains: Vec<String>,
    pub canonical_origin: Url,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostgresConfig {
    pub connection: PostgresConnection,
    pub pool_size: u32,
    pub ssl_mode: PostgresSslMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PostgresConnection {
    Url {
        source: PostgresUrlSource,
        url: SecretString,
    },
    Tcp {
        host: String,
        port: u16,
        database: String,
        username: String,
        password: SecretString,
    },
    UnixSocket {
        directory: PathBuf,
        port: u16,
        database: String,
        username: String,
        password: SecretString,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresUrlSource {
    PrimaryDatabaseUrl,
    DatabaseUrl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresSslMode {
    Disable,
    Allow,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaperclipConfig {
    pub root_path: PathBuf,
    pub root_url: PaperclipRootUrl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaperclipRootUrl {
    RootRelative(String),
    Absolute(Url),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigWarning {
    SmtpDisabled,
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SmtpDisabled => formatter
                .write_str("SMTP_SERVER is not configured; outbound email will be unavailable"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SmtpConfig {
    Disabled { warning: ConfigWarning },
    Enabled(Box<SmtpSettings>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SmtpSettings {
    pub delivery_method: SmtpDeliveryMethod,
    pub server: String,
    pub port: u16,
    pub login: Option<SecretString>,
    pub password: Option<SecretString>,
    pub from: Mailbox,
    pub reply_to: Option<Mailbox>,
    pub return_path: Option<String>,
    pub domain: String,
    pub authentication: SmtpAuthentication,
    pub transport: SmtpTransport,
    pub verify_mode: Option<SmtpVerifyMode>,
    pub ca_file: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmtpDeliveryMethod {
    Smtp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mailbox {
    pub display_name: Option<String>,
    pub address: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmtpAuthentication {
    None,
    Plain,
    Login,
    CramMd5,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmtpTransport {
    Plain,
    StartTls(StartTlsMode),
    Tls,
    Ssl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartTlsMode {
    Opportunistic,
    Required,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmtpVerifyMode {
    None,
    Peer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CryptographicSecrets {
    pub secret_key_base: SecretString,
    pub active_record_encryption: ActiveRecordEncryptionSecrets,
    pub vapid: Option<VapidKeyPair>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveRecordEncryptionSecrets {
    pub primary_key: SecretString,
    pub deterministic_key: SecretString,
    pub key_derivation_salt: SecretString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VapidKeyPair {
    pub private_key: SecretString,
    pub public_key: SecretString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedConfiguration {
    pub object_storage: BTreeSet<ObjectStorageProvider>,
    pub external_auth: BTreeSet<ExternalAuthProvider>,
    pub omniauth_only: bool,
    pub one_click_sso_login: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ObjectStorageProvider {
    S3,
    Swift,
    Azure,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExternalAuthProvider {
    Ldap,
    Pam,
    Cas,
    Saml,
    Oidc,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RedisEndpoint {
    Url(SecretString),
    Tcp {
        host: String,
        port: u16,
        database: u32,
        username: Option<SecretString>,
        password: Option<SecretString>,
    },
}

/// An environment validation failure that never stores the supplied value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigError {
    variables: Vec<&'static str>,
    detail: &'static str,
}

impl ConfigError {
    fn new(variables: &[&'static str], detail: &'static str) -> Self {
        Self {
            variables: variables.to_vec(),
            detail,
        }
    }

    /// Returns the environment variables responsible for this failure.
    #[must_use]
    pub fn variables(&self) -> &[&'static str] {
        &self.variables
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid configuration for {}: {}",
            self.variables.join(", "),
            self.detail
        )
    }
}

impl std::error::Error for ConfigError {}

fn parse_domains(environment: &HashMap<String, String>) -> Result<DomainConfig, ConfigError> {
    let local_domain = parse_domain(required_value(environment, "LOCAL_DOMAIN")?, "LOCAL_DOMAIN")?;
    let web_domain = environment.get("WEB_DOMAIN").map_or_else(
        || Ok(local_domain.clone()),
        |value| parse_domain(value, "WEB_DOMAIN"),
    )?;
    let alternate_domains = environment
        .get("ALTERNATE_DOMAINS")
        .map_or_else(|| Ok(Vec::new()), |value| parse_alternate_domains(value))?;
    let canonical_origin = Url::parse(&format!("https://{web_domain}"))
        .map_err(|_| ConfigError::new(&["WEB_DOMAIN"], "cannot form a canonical HTTPS origin"))?;

    Ok(DomainConfig {
        local_domain,
        web_domain,
        alternate_domains,
        canonical_origin,
    })
}

fn parse_alternate_domains(value: &str) -> Result<Vec<String>, ConfigError> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    let mut domains = Vec::new();

    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(ConfigError::new(
                &["ALTERNATE_DOMAINS"],
                "must not contain empty entries",
            ));
        }
        let domain = parse_domain(entry, "ALTERNATE_DOMAINS")?;
        if seen.insert(domain.clone()) {
            domains.push(domain);
        }
    }

    Ok(domains)
}

fn parse_domain(value: &str, variable: &'static str) -> Result<String, ConfigError> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(ConfigError::new(
            &[variable],
            "must be a nonempty domain without whitespace",
        ));
    }

    let parsed = Url::parse(&format!("https://{value}"))
        .map_err(|_| ConfigError::new(&[variable], "must be a valid bare domain"))?;
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ConfigError::new(
            &[variable],
            "must not include a scheme, user information, path, query, or fragment",
        ));
    }
    if parsed.port() == Some(0) {
        return Err(ConfigError::new(&[variable], "must not use port zero"));
    }
    authority(&parsed).ok_or_else(|| ConfigError::new(&[variable], "must contain a valid host"))
}

fn authority(url: &Url) -> Option<String> {
    let host = normalized_host(&url.host()?)?;
    Some(
        url.port()
            .map_or(host.clone(), |port| format!("{host}:{port}")),
    )
}

fn normalized_host(host: &Host<&str>) -> Option<String> {
    match host {
        Host::Domain(domain) if !domain.is_empty() => Some((*domain).to_owned()),
        Host::Ipv4(address) => Some(address.to_string()),
        Host::Ipv6(address) => Some(format!("[{address}]")),
        Host::Domain(_) => None,
    }
}

fn reject_replica_configuration(environment: &HashMap<String, String>) -> Result<(), ConfigError> {
    if let Some(variable) = REPLICA_VARIABLES
        .iter()
        .find(|variable| environment.contains_key(**variable))
    {
        return Err(ConfigError::new(
            &[*variable],
            "an explicit PostgreSQL read replica is unsupported in v1",
        ));
    }
    Ok(())
}

fn parse_database(environment: &HashMap<String, String>) -> Result<PostgresConfig, ConfigError> {
    let pool_variable = if environment.contains_key("DB_POOL") {
        "DB_POOL"
    } else if environment.contains_key("MAX_THREADS") {
        "MAX_THREADS"
    } else {
        "DB_POOL"
    };
    let pool_size = environment
        .get("DB_POOL")
        .or_else(|| environment.get("MAX_THREADS"))
        .map_or(Ok(5), |value| parse_positive_u32(value, pool_variable))?;

    if let Some(url) = environment.get("PRIMARY_DATABASE_URL") {
        let (connection, ssl_mode) = parse_database_url(
            url,
            "PRIMARY_DATABASE_URL",
            PostgresUrlSource::PrimaryDatabaseUrl,
            environment,
        )?;
        return Ok(PostgresConfig {
            connection,
            pool_size,
            ssl_mode,
        });
    }
    if let Some(url) = environment.get("DATABASE_URL") {
        let (connection, ssl_mode) = parse_database_url(
            url,
            "DATABASE_URL",
            PostgresUrlSource::DatabaseUrl,
            environment,
        )?;
        return Ok(PostgresConfig {
            connection,
            pool_size,
            ssl_mode,
        });
    }

    let host = environment
        .get("DB_HOST")
        .map_or("localhost", String::as_str);
    if host.is_empty() {
        return Err(ConfigError::new(&["DB_HOST"], "must not be empty"));
    }
    let port = environment
        .get("DB_PORT")
        .map_or(Ok(5432), |value| parse_port(value, "DB_PORT"))?;
    let database = nonempty_or_default(environment, "DB_NAME", "mastodon_production")?;
    let username = nonempty_or_default(environment, "DB_USER", "mastodon")?;
    let password = SecretString::new(environment.get("DB_PASS").cloned().unwrap_or_default());
    let ssl_mode = environment
        .get("DB_SSLMODE")
        .map_or(Ok(PostgresSslMode::Prefer), |value| {
            parse_postgres_ssl_mode(value, "DB_SSLMODE")
        })?;

    let connection = if Path::new(host).is_absolute() {
        PostgresConnection::UnixSocket {
            directory: PathBuf::from(host),
            port,
            database,
            username,
            password,
        }
    } else {
        PostgresConnection::Tcp {
            host: parse_host(host, "DB_HOST")?,
            port,
            database,
            username,
            password,
        }
    };

    Ok(PostgresConfig {
        connection,
        pool_size,
        ssl_mode,
    })
}

fn parse_database_url(
    value: &str,
    variable: &'static str,
    source: PostgresUrlSource,
    environment: &HashMap<String, String>,
) -> Result<(PostgresConnection, PostgresSslMode), ConfigError> {
    if value.is_empty()
        || value.chars().any(char::is_whitespace)
        || has_invalid_percent_encoding(value)
    {
        return Err(ConfigError::new(
            &[variable],
            "must be a nonempty PostgreSQL URL without unescaped whitespace",
        ));
    }
    let parsed = Url::parse(value)
        .map_err(|_| ConfigError::new(&[variable], "must be a valid PostgreSQL URL"))?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") || parsed.fragment().is_some() {
        return Err(ConfigError::new(
            &[variable],
            "must use postgres or postgresql and must not contain a fragment",
        ));
    }

    let socket_hosts = parsed
        .query_pairs()
        .filter_map(|(name, value)| (name == "host").then_some(value))
        .collect::<Vec<_>>();
    if socket_hosts.len() > 1 {
        return Err(ConfigError::new(
            &[variable],
            "must not contain multiple host query parameters",
        ));
    }
    if parsed.host().is_none() {
        let Some(host) = socket_hosts.first() else {
            return Err(ConfigError::new(
                &[variable],
                "must identify a TCP host or Unix socket directory",
            ));
        };
        if !Path::new(host.as_ref()).is_absolute() {
            return Err(ConfigError::new(
                &[variable],
                "a Unix socket host must be an absolute path",
            ));
        }
    }

    let ssl_modes = parsed
        .query_pairs()
        .filter_map(|(name, value)| (name == "sslmode").then_some(value))
        .collect::<Vec<_>>();
    if ssl_modes.len() > 1 {
        return Err(ConfigError::new(
            &[variable],
            "must not contain multiple sslmode query parameters",
        ));
    }
    let ssl_mode = if let Some(value) = ssl_modes.first() {
        parse_postgres_ssl_mode(value, variable)?
    } else {
        environment
            .get("DB_SSLMODE")
            .map_or(Ok(PostgresSslMode::Prefer), |value| {
                parse_postgres_ssl_mode(value, "DB_SSLMODE")
            })?
    };

    Ok((
        PostgresConnection::Url {
            source,
            url: SecretString::new(value.to_owned()),
        },
        ssl_mode,
    ))
}

fn parse_postgres_ssl_mode(
    value: &str,
    variable: &'static str,
) -> Result<PostgresSslMode, ConfigError> {
    match value {
        "disable" => Ok(PostgresSslMode::Disable),
        "allow" => Ok(PostgresSslMode::Allow),
        "prefer" => Ok(PostgresSslMode::Prefer),
        "require" => Ok(PostgresSslMode::Require),
        "verify-ca" => Ok(PostgresSslMode::VerifyCa),
        "verify-full" => Ok(PostgresSslMode::VerifyFull),
        _ => Err(ConfigError::new(
            &[variable],
            "contains an unsupported sslmode",
        )),
    }
}

fn parse_paperclip(environment: &HashMap<String, String>) -> Result<PaperclipConfig, ConfigError> {
    let root_path = PathBuf::from(required_value(environment, "PAPERCLIP_ROOT_PATH")?);
    if !root_path.is_absolute() {
        return Err(ConfigError::new(
            &["PAPERCLIP_ROOT_PATH"],
            "must be an explicit absolute path",
        ));
    }
    let root_url = environment.get("PAPERCLIP_ROOT_URL").map_or_else(
        || Ok(PaperclipRootUrl::RootRelative("/system".to_owned())),
        |value| parse_paperclip_root_url(value),
    )?;

    Ok(PaperclipConfig {
        root_path,
        root_url,
    })
}

fn parse_paperclip_root_url(value: &str) -> Result<PaperclipRootUrl, ConfigError> {
    if value.starts_with('/') {
        if !is_clean_root_relative_path(value) {
            return Err(ConfigError::new(
                &["PAPERCLIP_ROOT_URL"],
                "must be a clean root-relative path",
            ));
        }
        return Ok(PaperclipRootUrl::RootRelative(value.to_owned()));
    }

    let parsed = Url::parse(value).map_err(|_| {
        ConfigError::new(
            &["PAPERCLIP_ROOT_URL"],
            "must be root-relative or an absolute HTTPS URL",
        )
    })?;
    if parsed.scheme() != "https"
        || parsed.host().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port() == Some(0)
    {
        return Err(ConfigError::new(
            &["PAPERCLIP_ROOT_URL"],
            "absolute media URLs must be clean HTTPS URLs without user information",
        ));
    }
    Ok(PaperclipRootUrl::Absolute(parsed))
}

fn is_clean_root_relative_path(value: &str) -> bool {
    if value.is_empty()
        || value.starts_with("//")
        || value.contains("//")
        || value.contains(['\\', '?', '#'])
        || value.chars().any(char::is_whitespace)
        || (value.len() > 1 && value.ends_with('/'))
    {
        return false;
    }
    let Ok(parsed) = Url::parse(&format!("https://paperclip.invalid{value}")) else {
        return false;
    };
    parsed.path() == value
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && value
            .split('/')
            .all(|component| !matches!(component, "." | ".."))
}

fn parse_trusted_proxies(
    environment: &HashMap<String, String>,
) -> Result<Vec<IpNetwork>, ConfigError> {
    let entries = if let Some(value) = environment.get("TRUSTED_PROXY_IP") {
        if value.trim().is_empty() {
            return Err(ConfigError::new(
                &["TRUSTED_PROXY_IP"],
                "must contain at least one address or CIDR",
            ));
        }
        let mut entries = Vec::new();
        for comma_group in value.split(',') {
            if comma_group.trim().is_empty() {
                return Err(ConfigError::new(
                    &["TRUSTED_PROXY_IP"],
                    "must not contain an empty comma-separated entry",
                ));
            }
            entries.extend(comma_group.split_whitespace());
        }
        entries
    } else {
        TRUSTED_PROXY_DEFAULTS.to_vec()
    };

    let mut seen = HashSet::new();
    let mut proxies = Vec::new();
    for entry in entries {
        let parsed = entry
            .parse::<IpNetwork>()
            .or_else(|_| entry.parse::<IpAddr>().map(IpNetwork::from).map_err(|_| ()));
        let network = parsed.map_err(|()| {
            ConfigError::new(
                &["TRUSTED_PROXY_IP"],
                "contains an invalid IP address or CIDR",
            )
        })?;
        let network = IpNetwork::new(network.network(), network.prefix()).map_err(|_| {
            ConfigError::new(
                &["TRUSTED_PROXY_IP"],
                "contains an invalid IP address or CIDR",
            )
        })?;
        if network.prefix() == 0 {
            return Err(ConfigError::new(
                &["TRUSTED_PROXY_IP"],
                "must not trust the entire IPv4 or IPv6 internet",
            ));
        }
        if seen.insert(network) {
            proxies.push(network);
        }
    }

    Ok(proxies)
}

fn parse_smtp(
    environment: &HashMap<String, String>,
    domains: &DomainConfig,
) -> Result<SmtpConfig, ConfigError> {
    let Some(server_value) = environment.get("SMTP_SERVER") else {
        return Ok(SmtpConfig::Disabled {
            warning: ConfigWarning::SmtpDisabled,
        });
    };
    if server_value.is_empty() {
        return Ok(SmtpConfig::Disabled {
            warning: ConfigWarning::SmtpDisabled,
        });
    }

    let server = parse_host(server_value, "SMTP_SERVER")?;
    let delivery_method = match environment
        .get("SMTP_DELIVERY_METHOD")
        .map_or("smtp", String::as_str)
    {
        "smtp" => SmtpDeliveryMethod::Smtp,
        _ => {
            return Err(ConfigError::new(
                &["SMTP_DELIVERY_METHOD"],
                "only smtp delivery is supported",
            ));
        }
    };
    let port = environment
        .get("SMTP_PORT")
        .map_or(Ok(25), |value| parse_port(value, "SMTP_PORT"))?;
    let login = optional_secret(environment, "SMTP_LOGIN");
    let password = optional_secret(environment, "SMTP_PASSWORD");
    let from = parse_mailbox(
        environment
            .get("SMTP_FROM_ADDRESS")
            .map_or("notifications@localhost", String::as_str),
        "SMTP_FROM_ADDRESS",
    )?;
    let reply_to = optional_mailbox(environment, "SMTP_REPLY_TO")?;
    let return_path = optional_return_path(environment)?;
    let default_domain = domain_host(&domains.local_domain, "LOCAL_DOMAIN")?;
    let domain = environment.get("SMTP_DOMAIN").map_or_else(
        || Ok(default_domain),
        |value| parse_host(value, "SMTP_DOMAIN"),
    )?;
    let authentication = parse_smtp_authentication(environment)?;
    if authentication == SmtpAuthentication::None && (login.is_some() || password.is_some()) {
        return Err(ConfigError::new(
            &["SMTP_AUTH_METHOD", "SMTP_LOGIN", "SMTP_PASSWORD"],
            "authentication none contradicts configured credentials",
        ));
    }
    if login.is_some() != password.is_some() {
        return Err(ConfigError::new(
            &["SMTP_LOGIN", "SMTP_PASSWORD"],
            "SMTP credentials must be configured together",
        ));
    }
    let transport = parse_smtp_transport(environment)?;
    let verify_mode = environment
        .get("SMTP_OPENSSL_VERIFY_MODE")
        .filter(|value| !value.is_empty())
        .map(|value| parse_smtp_verify_mode(value))
        .transpose()?;
    let ca_file = PathBuf::from(
        environment
            .get("SMTP_CA_FILE")
            .map_or("/etc/ssl/certs/ca-certificates.crt", String::as_str),
    );
    if !ca_file.is_absolute() {
        return Err(ConfigError::new(
            &["SMTP_CA_FILE"],
            "must be an absolute path",
        ));
    }

    Ok(SmtpConfig::Enabled(Box::new(SmtpSettings {
        delivery_method,
        server,
        port,
        login,
        password,
        from,
        reply_to,
        return_path,
        domain,
        authentication,
        transport,
        verify_mode,
        ca_file,
    })))
}

fn parse_smtp_authentication(
    environment: &HashMap<String, String>,
) -> Result<SmtpAuthentication, ConfigError> {
    match environment
        .get("SMTP_AUTH_METHOD")
        .map_or("plain", String::as_str)
    {
        "none" => Ok(SmtpAuthentication::None),
        "plain" => Ok(SmtpAuthentication::Plain),
        "login" => Ok(SmtpAuthentication::Login),
        "cram_md5" => Ok(SmtpAuthentication::CramMd5),
        _ => Err(ConfigError::new(
            &["SMTP_AUTH_METHOD"],
            "must be none, plain, login, or cram_md5",
        )),
    }
}

fn parse_smtp_transport(
    environment: &HashMap<String, String>,
) -> Result<SmtpTransport, ConfigError> {
    let tls = parse_optional_bool(environment, "SMTP_TLS")?.unwrap_or(false);
    let ssl = parse_optional_bool(environment, "SMTP_SSL")?.unwrap_or(false);
    let starttls_auto = parse_optional_bool(environment, "SMTP_ENABLE_STARTTLS_AUTO")?;
    let starttls = environment
        .get("SMTP_ENABLE_STARTTLS")
        .map(|value| match value.as_str() {
            "always" => Ok(Some(StartTlsMode::Required)),
            "auto" => Ok(Some(StartTlsMode::Opportunistic)),
            "never" | "false" => Ok(None),
            _ => Err(ConfigError::new(
                &["SMTP_ENABLE_STARTTLS"],
                "must be always, auto, never, or false",
            )),
        })
        .transpose()?;

    if tls && ssl {
        return Err(ConfigError::new(
            &["SMTP_TLS", "SMTP_SSL"],
            "direct TLS and SSL modes cannot both be enabled",
        ));
    }
    if tls || ssl {
        if starttls.flatten().is_some() {
            return Err(ConfigError::new(
                &["SMTP_TLS", "SMTP_SSL", "SMTP_ENABLE_STARTTLS"],
                "direct TLS or SSL cannot be combined with STARTTLS",
            ));
        }
        if starttls_auto == Some(true) {
            return Err(ConfigError::new(
                &["SMTP_TLS", "SMTP_SSL", "SMTP_ENABLE_STARTTLS_AUTO"],
                "direct TLS or SSL cannot be combined with automatic STARTTLS",
            ));
        }
        return Ok(if tls {
            SmtpTransport::Tls
        } else {
            SmtpTransport::Ssl
        });
    }

    let starttls = starttls.unwrap_or_else(|| {
        starttls_auto
            .unwrap_or(true)
            .then_some(StartTlsMode::Opportunistic)
    });
    Ok(starttls.map_or(SmtpTransport::Plain, SmtpTransport::StartTls))
}

fn parse_smtp_verify_mode(value: &str) -> Result<SmtpVerifyMode, ConfigError> {
    match value {
        "none" => Ok(SmtpVerifyMode::None),
        "peer" => Ok(SmtpVerifyMode::Peer),
        _ => Err(ConfigError::new(
            &["SMTP_OPENSSL_VERIFY_MODE"],
            "must be none or peer",
        )),
    }
}

fn parse_mailbox(value: &str, variable: &'static str) -> Result<Mailbox, ConfigError> {
    let value = value.trim();
    if value.is_empty() || value.contains(['\r', '\n']) {
        return Err(ConfigError::new(&[variable], "must be a valid mailbox"));
    }

    let (display_name, address) = if let Some(value) = value.strip_suffix('>') {
        let Some((display_name, address)) = value.split_once('<') else {
            return Err(ConfigError::new(&[variable], "must be a valid mailbox"));
        };
        let display_name = display_name.trim();
        let address = address.trim();
        if display_name.is_empty()
            || display_name.contains(['<', '>'])
            || address.contains(['<', '>'])
        {
            return Err(ConfigError::new(&[variable], "must be a valid mailbox"));
        }
        (Some(display_name.to_owned()), address)
    } else {
        if value.contains(['<', '>']) {
            return Err(ConfigError::new(&[variable], "must be a valid mailbox"));
        }
        (None, value)
    };

    let Some((local, domain)) = address.rsplit_once('@') else {
        return Err(ConfigError::new(&[variable], "must be a valid mailbox"));
    };
    if local.is_empty()
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || local.contains('@')
        || !local.chars().all(is_mailbox_local_character)
    {
        return Err(ConfigError::new(&[variable], "must be a valid mailbox"));
    }
    let domain = parse_host(domain, variable)?;

    Ok(Mailbox {
        display_name,
        address: format!("{local}@{domain}"),
    })
}

fn is_mailbox_local_character(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(
            character,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '/'
                | '='
                | '?'
                | '^'
                | '_'
                | '`'
                | '{'
                | '|'
                | '}'
                | '~'
        )
}

fn optional_mailbox(
    environment: &HashMap<String, String>,
    variable: &'static str,
) -> Result<Option<Mailbox>, ConfigError> {
    environment
        .get(variable)
        .filter(|value| !value.trim().is_empty())
        .map(|value| parse_mailbox(value, variable))
        .transpose()
}

fn optional_return_path(
    environment: &HashMap<String, String>,
) -> Result<Option<String>, ConfigError> {
    let Some(value) = environment
        .get("SMTP_RETURN_PATH")
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let mailbox = parse_mailbox(value, "SMTP_RETURN_PATH")?;
    if mailbox.display_name.is_some() {
        return Err(ConfigError::new(
            &["SMTP_RETURN_PATH"],
            "must be an address without a display name",
        ));
    }
    Ok(Some(mailbox.address))
}

fn parse_host(value: &str, variable: &'static str) -> Result<String, ConfigError> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(ConfigError::new(&[variable], "must be a valid host"));
    }
    let parsed = Url::parse(&format!("https://{value}"))
        .map_err(|_| ConfigError::new(&[variable], "must be a valid host"))?;
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ConfigError::new(
            &[variable],
            "must be a bare host without a port or path",
        ));
    }
    normalized_host(
        &parsed
            .host()
            .ok_or_else(|| ConfigError::new(&[variable], "must contain a host"))?,
    )
    .ok_or_else(|| ConfigError::new(&[variable], "must contain a host"))
}

fn domain_host(value: &str, variable: &'static str) -> Result<String, ConfigError> {
    let parsed = Url::parse(&format!("https://{value}"))
        .map_err(|_| ConfigError::new(&[variable], "must contain a valid host"))?;
    normalized_host(
        &parsed
            .host()
            .ok_or_else(|| ConfigError::new(&[variable], "must contain a valid host"))?,
    )
    .ok_or_else(|| ConfigError::new(&[variable], "must contain a valid host"))
}

fn parse_secrets(
    environment: &HashMap<String, String>,
) -> Result<CryptographicSecrets, ConfigError> {
    if environment.contains_key("SECRET_KEY_BASE_DUMMY") {
        return Err(ConfigError::new(
            &["SECRET_KEY_BASE_DUMMY"],
            "dummy secret keys are not safe for Rustodon",
        ));
    }

    let secret_key_base = required_secret(environment, "SECRET_KEY_BASE")?;
    let active_record_encryption = ActiveRecordEncryptionSecrets {
        primary_key: required_secret(environment, "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY")?,
        deterministic_key: required_secret(
            environment,
            "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY",
        )?,
        key_derivation_salt: required_secret(
            environment,
            "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT",
        )?,
    };
    for (variable, secret) in [
        (
            "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY",
            &active_record_encryption.primary_key,
        ),
        (
            "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY",
            &active_record_encryption.deterministic_key,
        ),
        (
            "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT",
            &active_record_encryption.key_derivation_salt,
        ),
    ] {
        if secret.expose_secret().ends_with("DO_NOT_USE_IN_PRODUCTION") {
            return Err(ConfigError::new(
                &[variable],
                "contains Mastodon's public test-only encryption value",
            ));
        }
    }
    let vapid_private = optional_secret(environment, "VAPID_PRIVATE_KEY");
    let vapid_public = optional_secret(environment, "VAPID_PUBLIC_KEY");
    let vapid = match (vapid_private, vapid_public) {
        (None, None) => None,
        (Some(private_key), Some(public_key)) => Some(VapidKeyPair {
            private_key,
            public_key,
        }),
        _ => {
            return Err(ConfigError::new(
                &["VAPID_PRIVATE_KEY", "VAPID_PUBLIC_KEY"],
                "VAPID keys must be configured together",
            ));
        }
    };

    Ok(CryptographicSecrets {
        secret_key_base,
        active_record_encryption,
        vapid,
    })
}

fn parse_unsupported_configuration(
    environment: &HashMap<String, String>,
) -> Result<UnsupportedConfiguration, ConfigError> {
    let mut object_storage = BTreeSet::new();
    for (variable, provider) in [
        ("S3_ENABLED", ObjectStorageProvider::S3),
        ("SWIFT_ENABLED", ObjectStorageProvider::Swift),
        ("AZURE_ENABLED", ObjectStorageProvider::Azure),
    ] {
        if parse_optional_bool(environment, variable)?.unwrap_or(false) {
            object_storage.insert(provider);
        }
    }

    let mut external_auth = BTreeSet::new();
    for (variable, provider) in [
        ("LDAP_ENABLED", ExternalAuthProvider::Ldap),
        ("PAM_ENABLED", ExternalAuthProvider::Pam),
        ("CAS_ENABLED", ExternalAuthProvider::Cas),
        ("SAML_ENABLED", ExternalAuthProvider::Saml),
        ("OIDC_ENABLED", ExternalAuthProvider::Oidc),
    ] {
        if parse_optional_bool(environment, variable)?.unwrap_or(false) {
            external_auth.insert(provider);
        }
    }

    Ok(UnsupportedConfiguration {
        object_storage,
        external_auth,
        omniauth_only: parse_optional_bool(environment, "OMNIAUTH_ONLY")?.unwrap_or(false),
        one_click_sso_login: parse_optional_bool(environment, "ONE_CLICK_SSO_LOGIN")?
            .unwrap_or(false),
    })
}

fn parse_sidekiq_redis(
    environment: &HashMap<String, String>,
) -> Result<Option<RedisEndpoint>, ConfigError> {
    if let Some(variable) = REDIS_SENTINEL_VARIABLES
        .iter()
        .find(|variable| environment.contains_key(**variable))
    {
        return Err(ConfigError::new(
            &[*variable],
            "Redis Sentinel is unsupported by preflight; provide a direct SIDEKIQ_REDIS_URL",
        ));
    }
    if let Some(value) = environment.get("SIDEKIQ_REDIS_URL") {
        return parse_redis_url(value, "SIDEKIQ_REDIS_URL").map(Some);
    }
    if environment
        .get("SIDEKIQ_REDIS_HOST")
        .is_some_and(|host| !host.is_empty())
    {
        return parse_discrete_redis(environment, true).map(Some);
    }

    if let Some(value) = environment.get("REDIS_URL") {
        return parse_redis_url(value, "REDIS_URL").map(Some);
    }
    parse_discrete_redis(environment, false).map(Some)
}

fn parse_redis_url(value: &str, variable: &'static str) -> Result<RedisEndpoint, ConfigError> {
    if value.is_empty()
        || value.chars().any(char::is_whitespace)
        || has_invalid_percent_encoding(value)
    {
        return Err(ConfigError::new(
            &[variable],
            "must be a nonempty Redis URL without unescaped whitespace",
        ));
    }
    let parsed = Url::parse(value)
        .map_err(|_| ConfigError::new(&[variable], "must be a valid Redis URL"))?;
    if !matches!(parsed.scheme(), "redis" | "rediss")
        || parsed.host().is_none()
        || parsed.fragment().is_some()
    {
        return Err(ConfigError::new(
            &[variable],
            "must use redis or rediss and identify a host",
        ));
    }
    Ok(RedisEndpoint::Url(SecretString::new(value.to_owned())))
}

fn parse_discrete_redis(
    environment: &HashMap<String, String>,
    sidekiq: bool,
) -> Result<RedisEndpoint, ConfigError> {
    let variable = |suffix: &'static str| -> &'static str {
        match (sidekiq, suffix) {
            (true, "HOST") => "SIDEKIQ_REDIS_HOST",
            (true, "PORT") => "SIDEKIQ_REDIS_PORT",
            (true, "DB") => "SIDEKIQ_REDIS_DB",
            (true, "USER") => "SIDEKIQ_REDIS_USER",
            (true, "PASSWORD") => "SIDEKIQ_REDIS_PASSWORD",
            (false, "HOST") => "REDIS_HOST",
            (false, "PORT") => "REDIS_PORT",
            (false, "DB") => "REDIS_DB",
            (false, "USER") => "REDIS_USER",
            (false, "PASSWORD") => "REDIS_PASSWORD",
            _ => unreachable!("known Redis variable suffix"),
        }
    };
    let host_variable = variable("HOST");
    let host = parse_host(
        environment
            .get(variable("HOST"))
            .map_or("localhost", String::as_str),
        host_variable,
    )?;
    let port_variable = variable("PORT");
    let port = environment
        .get(variable("PORT"))
        .map_or(Ok(6379), |value| parse_port(value, port_variable))?;
    let database_variable = variable("DB");
    let database = environment.get(variable("DB")).map_or(Ok(0), |value| {
        value
            .parse::<u32>()
            .map_err(|_| ConfigError::new(&[database_variable], "must be a nonnegative integer"))
    })?;
    let username = environment
        .get(variable("USER"))
        .filter(|value| !value.is_empty())
        .map(|value| SecretString::new(value.clone()));
    let password = environment
        .get(variable("PASSWORD"))
        .filter(|value| !value.is_empty())
        .map(|value| SecretString::new(value.clone()));

    Ok(RedisEndpoint::Tcp {
        host,
        port,
        database,
        username,
        password,
    })
}

fn required_value<'a>(
    environment: &'a HashMap<String, String>,
    variable: &'static str,
) -> Result<&'a str, ConfigError> {
    environment
        .get(variable)
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .ok_or_else(|| ConfigError::new(&[variable], "is required and must not be empty"))
}

fn required_secret(
    environment: &HashMap<String, String>,
    variable: &'static str,
) -> Result<SecretString, ConfigError> {
    let value = environment
        .get(variable)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ConfigError::new(&[variable], "is required and must not be empty"))?;
    Ok(SecretString::new(value.clone()))
}

fn optional_secret(
    environment: &HashMap<String, String>,
    variable: &'static str,
) -> Option<SecretString> {
    environment
        .get(variable)
        .filter(|value| !value.is_empty())
        .map(|value| SecretString::new(value.clone()))
}

fn nonempty_or_default(
    environment: &HashMap<String, String>,
    variable: &'static str,
    default: &str,
) -> Result<String, ConfigError> {
    environment.get(variable).map_or_else(
        || Ok(default.to_owned()),
        |value| {
            if value.is_empty() {
                Err(ConfigError::new(&[variable], "must not be empty"))
            } else {
                Ok(value.clone())
            }
        },
    )
}

fn parse_port(value: &str, variable: &'static str) -> Result<u16, ConfigError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| ConfigError::new(&[variable], "must be an integer from 1 through 65535"))
}

fn parse_positive_u32(value: &str, variable: &'static str) -> Result<u32, ConfigError> {
    value
        .parse::<u32>()
        .ok()
        .filter(|number| *number != 0)
        .ok_or_else(|| ConfigError::new(&[variable], "must be a positive integer"))
}

fn parse_optional_bool(
    environment: &HashMap<String, String>,
    variable: &'static str,
) -> Result<Option<bool>, ConfigError> {
    environment
        .get(variable)
        .map(|value| match value.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(ConfigError::new(
                &[variable],
                "must be exactly true or false",
            )),
        })
        .transpose()
}

fn has_invalid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return true;
        }
        index += 1;
    }
    false
}
