use std::fmt;
use std::fs;
use std::path::Path;

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use lettre::message::Mailbox as LettreMailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Certificate, CertificateStore, Tls, TlsParametersBuilder};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use rsa::rand_core::{OsRng, RngCore};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::config::{Mailbox, SmtpAuthentication, SmtpConfig, SmtpTransport, SmtpVerifyMode};
use crate::jobs::{ClaimedJob, JobSpec, Lane};
use crate::secret::SecretString;

pub const PASSWORD_RESET_JOB_KIND: &str = "rustodon.mail.reset_password";
pub const CONFIRMATION_JOB_KIND: &str = "rustodon.mail.confirmation";
pub const REPORT_JOB_KIND: &str = "rustodon.mail.new_report";

const DEFAULT_SMTP_CA_FILE: &str = "/etc/ssl/certs/ca-certificates.crt";
const MAIL_TOKEN_VERSION: u8 = 1;
const MAIL_TOKEN_NONCE_BYTES: usize = 12;
const MAIL_TOKEN_TAG_BYTES: usize = 16;

#[must_use]
pub fn report_job(
    recipient: &str,
    origin: &str,
    report_id: i64,
    target: &str,
    reporter: &str,
    staff_account_id: i64,
) -> JobSpec {
    JobSpec::new(
        Lane::Mail,
        REPORT_JOB_KIND,
        json!({
            "to": recipient,
            "origin": origin,
            "report_id": report_id,
            "target": target,
            "reporter": reporter,
        }),
    )
    .logical_key(format!("report-email:{report_id}:{staff_account_id}"))
}

#[derive(Debug)]
pub enum MailError {
    Disabled,
    InvalidJob(&'static str),
    InvalidAddress,
    InvalidToken,
    Encryption,
    Certificate,
    UnsupportedAuthentication,
    TransportConfiguration,
    Transport,
}

impl MailError {
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Transport)
    }
}

impl fmt::Display for MailError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("SMTP mail is disabled"),
            Self::InvalidJob(message) => formatter.write_str(message),
            Self::InvalidAddress => formatter.write_str("mail address is invalid"),
            Self::InvalidToken => formatter.write_str("mail token envelope is invalid"),
            Self::Encryption => formatter.write_str("mail token encryption failed"),
            Self::Certificate => formatter.write_str("SMTP certificate configuration is invalid"),
            Self::UnsupportedAuthentication => {
                formatter.write_str("SMTP authentication method is unsupported by the mail client")
            }
            Self::TransportConfiguration => {
                formatter.write_str("SMTP transport configuration is invalid")
            }
            Self::Transport => formatter.write_str("SMTP delivery failed"),
        }
    }
}

impl std::error::Error for MailError {}

#[derive(Clone)]
pub struct MailConfig {
    smtp: SmtpConfig,
    origin: Url,
    secret_key_base: SecretString,
}

impl MailConfig {
    #[must_use]
    pub fn new(smtp: SmtpConfig, origin: Url, secret_key_base: SecretString) -> Self {
        Self {
            smtp,
            origin,
            secret_key_base,
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        matches!(self.smtp, SmtpConfig::Enabled(_))
    }

    #[must_use]
    pub fn token_digest_secret(&self) -> &str {
        self.secret_key_base.expose_secret()
    }

    /// Builds a durable reset-mail job while keeping the token encrypted at rest.
    ///
    /// # Errors
    ///
    /// Returns an error when the recipient or token cannot be represented safely.
    pub fn password_reset_job(&self, recipient: &str, token: &str) -> Result<JobSpec, MailError> {
        self.token_job(PASSWORD_RESET_JOB_KIND, "password-reset", recipient, token)
    }

    /// Builds a durable confirmation-mail job while keeping the token encrypted at rest.
    ///
    /// # Errors
    ///
    /// Returns an error when the recipient or token cannot be represented safely.
    pub fn confirmation_job(&self, recipient: &str, token: &str) -> Result<JobSpec, MailError> {
        self.token_job(CONFIRMATION_JOB_KIND, "confirmation", recipient, token)
    }

    /// Creates a worker-side SMTP runtime when outbound mail is configured.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured SMTP transport cannot be built safely.
    pub fn runtime(&self) -> Result<Option<MailRuntime>, MailError> {
        let SmtpConfig::Enabled(settings) = &self.smtp else {
            return Ok(None);
        };
        Ok(Some(MailRuntime {
            sender: MailSender::new(settings)?,
            secret_key_base: self.secret_key_base.clone(),
        }))
    }

    fn token_job(
        &self,
        kind: &'static str,
        key_prefix: &'static str,
        recipient: &str,
        token: &str,
    ) -> Result<JobSpec, MailError> {
        let recipient = recipient.trim();
        if recipient.is_empty() || recipient.contains(['\r', '\n']) {
            return Err(MailError::InvalidAddress);
        }
        if token.is_empty() || token.contains(char::is_whitespace) {
            return Err(MailError::InvalidToken);
        }
        let sealed_token = seal_token(kind, token, &self.secret_key_base)?;
        let digest = Sha256::digest(token.as_bytes());
        Ok(JobSpec::new(
            Lane::Mail,
            kind,
            json!({
                "to": recipient,
                "origin": self.origin.as_str(),
                "token": sealed_token,
            }),
        )
        .logical_key(format!("{key_prefix}:{digest:x}")))
    }
}

#[derive(Clone)]
pub struct MailRuntime {
    sender: MailSender,
    secret_key_base: SecretString,
}

impl MailRuntime {
    /// Sends one claimed mail job.
    ///
    /// # Errors
    ///
    /// Returns a permanent error for malformed durable arguments and a retryable error for an
    /// SMTP delivery failure.
    pub async fn send(&self, job: &ClaimedJob) -> Result<(), MailError> {
        let message = self.message(&job.kind, &job.arguments)?;
        self.sender
            .transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|_| MailError::Transport)
    }

    fn message(&self, kind: &str, arguments: &Value) -> Result<Message, MailError> {
        let object = arguments.as_object().ok_or(MailError::InvalidJob(
            "mail job arguments must be an object",
        ))?;
        let recipient = object
            .get("to")
            .and_then(Value::as_str)
            .ok_or(MailError::InvalidJob("mail job recipient is missing"))?;
        let origin = object
            .get("origin")
            .and_then(Value::as_str)
            .ok_or(MailError::InvalidJob("mail job origin is missing"))?;
        let origin =
            Url::parse(origin).map_err(|_| MailError::InvalidJob("mail job origin is invalid"))?;
        if !matches!(origin.scheme(), "http" | "https") || origin.host().is_none() {
            return Err(MailError::InvalidJob("mail job origin is invalid"));
        }
        let (subject, body) = match kind {
            PASSWORD_RESET_JOB_KIND | CONFIRMATION_JOB_KIND => {
                let sealed_token = object
                    .get("token")
                    .and_then(Value::as_str)
                    .ok_or(MailError::InvalidJob("mail job token is missing"))?;
                let token = open_token(kind, sealed_token, &self.secret_key_base)?;
                let (subject, link_path, query_key, body_prefix) = match kind {
                    PASSWORD_RESET_JOB_KIND => (
                        "Reset your password",
                        "auth/password/edit",
                        "reset_password_token",
                        "Choose a new password using this link:",
                    ),
                    CONFIRMATION_JOB_KIND => (
                        "Confirm your account",
                        "auth/confirmation",
                        "confirmation_token",
                        "Confirm your account using this link:",
                    ),
                    _ => unreachable!(),
                };
                let mut link = origin
                    .join(link_path)
                    .map_err(|_| MailError::InvalidJob("mail job link could not be built"))?;
                link.query_pairs_mut()
                    .append_pair(query_key, token.expose_secret());
                let body = format!(
                    "{body_prefix}\n\n{link}\n\nIf you did not request this message, you can ignore it.\n"
                );
                (subject, body)
            }
            REPORT_JOB_KIND => {
                let report_id = object
                    .get("report_id")
                    .and_then(Value::as_i64)
                    .filter(|id| *id > 0)
                    .ok_or(MailError::InvalidJob("report mail ID is missing"))?;
                let target = object
                    .get("target")
                    .and_then(Value::as_str)
                    .ok_or(MailError::InvalidJob("report mail target is missing"))?;
                let reporter = object
                    .get("reporter")
                    .and_then(Value::as_str)
                    .ok_or(MailError::InvalidJob("report mail reporter is missing"))?;
                (
                    "New report",
                    format!(
                        "A new report was submitted for {target} by {reporter}.\n\nReport ID: {report_id}\n"
                    ),
                )
            }
            _ => return Err(MailError::InvalidJob("unknown mail job kind")),
        };
        let recipient = mailbox(recipient)?;
        let mut builder = Message::builder()
            .from(self.sender.from.clone())
            .to(recipient.clone())
            .subject(subject)
            .header(ContentType::TEXT_PLAIN);
        if let Some(reply_to) = &self.sender.reply_to {
            builder = builder.reply_to(reply_to.clone());
        }
        let return_path = self
            .sender
            .return_path
            .as_ref()
            .map_or_else(|| self.sender.from.email.to_string(), Clone::clone);
        let envelope = lettre::address::Envelope::new(
            Some(return_path.parse().map_err(|_| MailError::InvalidAddress)?),
            vec![recipient.email],
        )
        .map_err(|_| MailError::InvalidAddress)?;
        builder
            .envelope(envelope)
            .body(body)
            .map_err(|_| MailError::InvalidAddress)
    }
}

#[derive(Clone)]
struct MailSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: LettreMailbox,
    reply_to: Option<LettreMailbox>,
    return_path: Option<String>,
}

impl MailSender {
    fn new(settings: &crate::config::SmtpSettings) -> Result<Self, MailError> {
        let tls_parameters = matches!(
            settings.transport,
            SmtpTransport::StartTls(_) | SmtpTransport::Tls | SmtpTransport::Ssl
        )
        .then(|| tls_parameters(settings))
        .transpose()?;
        let mut builder =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(settings.server.clone())
                .port(settings.port)
                .hello_name(ClientId::Domain(settings.domain.clone()));
        if let Some(parameters) = tls_parameters {
            builder = builder.tls(match settings.transport {
                SmtpTransport::StartTls(crate::config::StartTlsMode::Opportunistic) => {
                    Tls::Opportunistic(parameters)
                }
                SmtpTransport::StartTls(crate::config::StartTlsMode::Required) => {
                    Tls::Required(parameters)
                }
                SmtpTransport::Tls | SmtpTransport::Ssl => Tls::Wrapper(parameters),
                SmtpTransport::Plain => return Err(MailError::TransportConfiguration),
            });
        } else if !matches!(settings.transport, SmtpTransport::Plain) {
            return Err(MailError::TransportConfiguration);
        }
        if let (Some(login), Some(password)) = (&settings.login, &settings.password) {
            let mechanism = match settings.authentication {
                SmtpAuthentication::Plain => Mechanism::Plain,
                SmtpAuthentication::Login => Mechanism::Login,
                SmtpAuthentication::CramMd5 => return Err(MailError::UnsupportedAuthentication),
                SmtpAuthentication::None => return Err(MailError::TransportConfiguration),
            };
            builder = builder
                .credentials(Credentials::new(
                    login.expose_secret().to_owned(),
                    password.expose_secret().to_owned(),
                ))
                .authentication(vec![mechanism]);
        }
        Ok(Self {
            transport: builder.build(),
            from: mailbox_from_config(&settings.from)?,
            reply_to: settings
                .reply_to
                .as_ref()
                .map(mailbox_from_config)
                .transpose()?,
            return_path: settings.return_path.clone(),
        })
    }
}

fn tls_parameters(
    settings: &crate::config::SmtpSettings,
) -> Result<lettre::transport::smtp::client::TlsParameters, MailError> {
    let mut builder = TlsParametersBuilder::new(settings.server.clone())
        .certificate_store(CertificateStore::Default);
    if settings.ca_file != Path::new(DEFAULT_SMTP_CA_FILE) {
        let pem = fs::read(&settings.ca_file).map_err(|_| MailError::Certificate)?;
        let certificate = Certificate::from_pem(&pem).map_err(|_| MailError::Certificate)?;
        builder = builder.add_root_certificate(certificate);
    }
    if settings.verify_mode == Some(SmtpVerifyMode::None) {
        builder = builder
            .dangerous_accept_invalid_certs(true)
            .dangerous_accept_invalid_hostnames(true);
    }
    builder.build().map_err(|_| MailError::Certificate)
}

fn mailbox_from_config(mailbox: &Mailbox) -> Result<LettreMailbox, MailError> {
    let value = mailbox.display_name.as_ref().map_or_else(
        || mailbox.address.clone(),
        |name| format!("{name} <{}>", mailbox.address),
    );
    value.parse().map_err(|_| MailError::InvalidAddress)
}

fn mailbox(value: &str) -> Result<LettreMailbox, MailError> {
    value.parse().map_err(|_| MailError::InvalidAddress)
}

fn seal_token(kind: &str, token: &str, secret: &SecretString) -> Result<String, MailError> {
    let key = Sha256::digest(secret.expose_secret().as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| MailError::Encryption)?;
    let mut nonce = [0_u8; MAIL_TOKEN_NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce);
    let mut ciphertext = token.as_bytes().to_vec();
    let tag = cipher
        .encrypt_in_place_detached(
            Nonce::from_slice(&nonce),
            token_aad(kind).as_bytes(),
            &mut ciphertext,
        )
        .map_err(|_| MailError::Encryption)?;
    let mut envelope =
        Vec::with_capacity(1 + MAIL_TOKEN_NONCE_BYTES + ciphertext.len() + MAIL_TOKEN_TAG_BYTES);
    envelope.push(MAIL_TOKEN_VERSION);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);
    envelope.extend_from_slice(&tag);
    Ok(URL_SAFE_NO_PAD.encode(envelope))
}

fn open_token(
    kind: &str,
    sealed_token: &str,
    secret: &SecretString,
) -> Result<SecretString, MailError> {
    let envelope = URL_SAFE_NO_PAD
        .decode(sealed_token)
        .map_err(|_| MailError::InvalidToken)?;
    if envelope.len() < 1 + MAIL_TOKEN_NONCE_BYTES + MAIL_TOKEN_TAG_BYTES
        || envelope[0] != MAIL_TOKEN_VERSION
    {
        return Err(MailError::InvalidToken);
    }
    let nonce_start = 1;
    let ciphertext_start = nonce_start + MAIL_TOKEN_NONCE_BYTES;
    let tag_start = envelope.len() - MAIL_TOKEN_TAG_BYTES;
    let nonce = &envelope[nonce_start..ciphertext_start];
    let mut ciphertext = envelope[ciphertext_start..tag_start].to_vec();
    let tag = Tag::from_slice(&envelope[tag_start..]);
    let key = Sha256::digest(secret.expose_secret().as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| MailError::Encryption)?;
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(nonce),
            token_aad(kind).as_bytes(),
            &mut ciphertext,
            tag,
        )
        .map_err(|_| MailError::InvalidToken)?;
    let token = String::from_utf8(ciphertext).map_err(|_| MailError::InvalidToken)?;
    Ok(SecretString::new(token))
}

fn token_aad(kind: &str) -> String {
    format!("rustodon-mail-token-v{MAIL_TOKEN_VERSION}:{kind}")
}
