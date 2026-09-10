use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};
use rustodon::config::{
    ConfigWarning, Mailbox, SmtpAuthentication, SmtpConfig, SmtpDeliveryMethod, SmtpSettings,
    SmtpTransport,
};
use rustodon::jobs::{ClaimedJob, Lane};
use rustodon::mail::MailError;
use rustodon::mail::{
    CONFIRMATION_JOB_KIND, MailConfig, PASSWORD_RESET_JOB_KIND, REPORT_JOB_KIND, report_job,
};
use rustodon::secret::SecretString;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use url::Url;

fn mail_config() -> MailConfig {
    MailConfig::new(
        SmtpConfig::Disabled {
            warning: ConfigWarning::SmtpDisabled,
        },
        Url::parse("https://example.invalid/").unwrap(),
        SecretString::new("mail-secret".to_owned()),
    )
}

#[test]
fn password_reset_job_encrypts_the_token_and_keeps_mail_metadata_durable() {
    let token = "password-reset-token";
    let job = mail_config()
        .password_reset_job("person@example.invalid", token)
        .unwrap();

    assert_eq!(job.lane(), Lane::Mail);
    assert_eq!(job.kind(), PASSWORD_RESET_JOB_KIND);
    assert_eq!(job.arguments()["to"], "person@example.invalid");
    assert_ne!(job.arguments().to_string(), token);
    assert_eq!(job.arguments()["origin"], "https://example.invalid/");
    assert!(job.arguments()["token"].as_str().is_some());
    assert!(
        job.arguments()["message_id_local"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("rustodon-mail-") && identity.len() == 46)
    );
    assert_eq!(job.arguments()["message_id_domain"], "example.invalid");
}

#[test]
fn confirmation_job_uses_a_distinct_mail_kind_and_never_persists_plaintext() {
    let token = "confirmation-token";
    let job = mail_config()
        .confirmation_job("person@example.invalid", token)
        .unwrap();

    assert_eq!(job.lane(), Lane::Mail);
    assert_eq!(job.kind(), CONFIRMATION_JOB_KIND);
    assert_ne!(job.arguments().to_string(), token);
    assert!(job.arguments()["message_id_local"].as_str().is_some());
}

#[test]
fn mail_config_reports_no_enabled_transport_when_smtp_is_disabled() {
    assert!(!mail_config().is_enabled());
}

#[test]
fn mail_jobs_use_unique_digest_keys_without_exposing_the_token() {
    let first = mail_config()
        .password_reset_job("person@example.invalid", "first-token")
        .unwrap();
    let second = mail_config()
        .password_reset_job("person@example.invalid", "second-token")
        .unwrap();

    assert_ne!(first.logical_key_value(), second.logical_key_value());
    assert!(
        first
            .logical_key_value()
            .is_some_and(|key| key.starts_with("password-reset:"))
    );
    assert!(
        first
            .logical_key_value()
            .is_some_and(|key| !key.contains("first-token"))
    );
    let first_identity = first.arguments()["message_id_local"].as_str().unwrap();
    let second_identity = second.arguments()["message_id_local"].as_str().unwrap();
    assert_ne!(first_identity, second_identity);
    assert!(!first_identity.contains("person"));
    assert!(!first_identity.contains("first-token"));
}

#[test]
fn report_mail_job_preserves_staff_context_without_a_token() {
    let job = report_job(
        "moderator@example.invalid",
        "https://example.invalid/",
        42,
        "target@example.invalid",
        "reporter@example.invalid",
        7,
    );

    assert_eq!(job.lane(), Lane::Mail);
    assert_eq!(job.kind(), REPORT_JOB_KIND);
    assert_eq!(job.arguments()["to"], "moderator@example.invalid");
    assert_eq!(job.arguments()["report_id"], 42);
    assert_eq!(job.arguments()["target"], "target@example.invalid");
    assert_eq!(job.arguments()["reporter"], "reporter@example.invalid");
    assert!(job.arguments().get("token").is_none());
    assert!(job.arguments()["message_id_local"].as_str().is_some());
    assert_eq!(job.arguments()["message_id_domain"], "example.invalid");
    assert_eq!(job.logical_key_value(), Some("report-email:42:7"));
}

#[test]
fn mail_lane_is_available_in_the_full_lane_set() {
    let lanes = Lane::ALL.into_iter().collect::<BTreeSet<_>>();
    assert!(lanes.contains(&Lane::Mail));
}

#[tokio::test]
async fn a_refused_smtp_connection_is_retryable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config = MailConfig::new(
        SmtpConfig::Enabled(Box::new(SmtpSettings {
            delivery_method: SmtpDeliveryMethod::Smtp,
            server: "127.0.0.1".to_owned(),
            port,
            login: None,
            password: None,
            from: Mailbox {
                display_name: None,
                address: "notifications@example.invalid".to_owned(),
            },
            reply_to: None,
            return_path: None,
            domain: "example.invalid".to_owned(),
            authentication: SmtpAuthentication::None,
            transport: SmtpTransport::Plain,
            verify_mode: None,
            ca_file: PathBuf::from("/etc/ssl/certs/ca-certificates.crt"),
        })),
        Url::parse("https://example.invalid/").unwrap(),
        SecretString::new("mail-secret".to_owned()),
    );
    let job = config
        .password_reset_job("person@example.invalid", "password-reset-token")
        .unwrap();
    let claimed = ClaimedJob {
        id: 1,
        lane: job.lane(),
        kind: job.kind().to_owned(),
        arguments: job.arguments().clone(),
        logical_key: job.logical_key_value().map(str::to_owned),
        run_at: Utc::now(),
        attempt: 1,
        max_attempts: 25,
        generation: 1,
        lease_owner: "mail-test".to_owned(),
        lease_expires_at: Utc::now() + Duration::minutes(1),
    };
    let runtime = config.runtime().unwrap().unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), runtime.send(&claimed))
        .await
        .unwrap();

    assert!(matches!(result, Err(MailError::Transport)));
    assert!(MailError::Transport.retryable());
}

#[tokio::test]
async fn malformed_or_missing_persisted_message_identity_fails_closed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config = MailConfig::new(
        SmtpConfig::Enabled(Box::new(SmtpSettings {
            delivery_method: SmtpDeliveryMethod::Smtp,
            server: "127.0.0.1".to_owned(),
            port,
            login: None,
            password: None,
            from: Mailbox {
                display_name: None,
                address: "notifications@example.invalid".to_owned(),
            },
            reply_to: None,
            return_path: None,
            domain: "changed-smtp.example.invalid".to_owned(),
            authentication: SmtpAuthentication::None,
            transport: SmtpTransport::Plain,
            verify_mode: None,
            ca_file: PathBuf::from("/etc/ssl/certs/ca-certificates.crt"),
        })),
        Url::parse("https://example.invalid/").unwrap(),
        SecretString::new("mail-secret".to_owned()),
    );
    let job = config
        .password_reset_job("person@example.invalid", "password-reset-token")
        .unwrap();
    let runtime = config.runtime().unwrap().unwrap();

    for arguments in [
        {
            let mut arguments = job.arguments().clone();
            arguments
                .as_object_mut()
                .unwrap()
                .remove("message_id_local");
            arguments
        },
        {
            let mut arguments = job.arguments().clone();
            arguments["message_id_local"] = serde_json::json!("recipient@example.invalid");
            arguments
        },
        {
            let mut arguments = job.arguments().clone();
            arguments["message_id_domain"] = serde_json::json!("bad domain\r\nBcc: leak");
            arguments
        },
    ] {
        let claimed = ClaimedJob {
            id: 99,
            lane: job.lane(),
            kind: job.kind().to_owned(),
            arguments,
            logical_key: job.logical_key_value().map(str::to_owned),
            run_at: Utc::now(),
            attempt: 1,
            max_attempts: 25,
            generation: 1,
            lease_owner: "mail-test".to_owned(),
            lease_expires_at: Utc::now() + Duration::minutes(1),
        };
        assert!(matches!(
            runtime.send(&claimed).await,
            Err(MailError::InvalidJob(_))
        ));
    }
}

#[tokio::test]
async fn worker_delivers_a_decrypted_reset_message_through_smtp() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(fixture_smtp_server(listener));

    let config = MailConfig::new(
        SmtpConfig::Enabled(Box::new(SmtpSettings {
            delivery_method: SmtpDeliveryMethod::Smtp,
            server: "127.0.0.1".to_owned(),
            port,
            login: None,
            password: None,
            from: Mailbox {
                display_name: Some("Fixture Notifications".to_owned()),
                address: "notifications@example.invalid".to_owned(),
            },
            reply_to: None,
            return_path: None,
            domain: "example.invalid".to_owned(),
            authentication: SmtpAuthentication::None,
            transport: SmtpTransport::Plain,
            verify_mode: None,
            ca_file: PathBuf::from("/etc/ssl/certs/ca-certificates.crt"),
        })),
        Url::parse("https://example.invalid/").unwrap(),
        SecretString::new("mail-secret".to_owned()),
    );
    let job = config
        .password_reset_job("person@example.invalid", "password-reset-token")
        .unwrap();
    let expected_message_id = format!(
        "Message-ID: <{}@{}>",
        job.arguments()["message_id_local"].as_str().unwrap(),
        job.arguments()["message_id_domain"].as_str().unwrap()
    );
    let claimed = ClaimedJob {
        id: 1,
        lane: job.lane(),
        kind: job.kind().to_owned(),
        arguments: job.arguments().clone(),
        logical_key: job.logical_key_value().map(str::to_owned),
        run_at: Utc::now(),
        attempt: 1,
        max_attempts: 25,
        generation: 1,
        lease_owner: "mail-test".to_owned(),
        lease_expires_at: Utc::now() + Duration::minutes(1),
    };
    let runtime = config.runtime().unwrap().unwrap();
    runtime.send(&claimed).await.unwrap();

    let message = tokio::time::timeout(StdDuration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let message = String::from_utf8(message).unwrap();
    assert!(
        message.contains("Subject: Reset your password"),
        "{message:?}"
    );
    assert!(
        message.contains("To: person@example.invalid"),
        "{message:?}"
    );
    assert!(message.contains(&expected_message_id), "{message:?}");
    assert!(!message.contains("rustodon-mail-1@"), "{message:?}");
    assert!(!message.contains("mail-secret"), "{message:?}");
    assert!(
        message.contains(
            "https://example.invalid/auth/password/edit?reset_password_token=3Dpassword-=\r\nreset-token"
        ),
        "{message:?}"
    );
}

#[tokio::test]
async fn worker_delivers_a_report_message_through_smtp() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(fixture_smtp_server(listener));

    let config = MailConfig::new(
        SmtpConfig::Enabled(Box::new(SmtpSettings {
            delivery_method: SmtpDeliveryMethod::Smtp,
            server: "127.0.0.1".to_owned(),
            port,
            login: None,
            password: None,
            from: Mailbox {
                display_name: Some("Fixture Notifications".to_owned()),
                address: "notifications@example.invalid".to_owned(),
            },
            reply_to: None,
            return_path: None,
            domain: "example.invalid".to_owned(),
            authentication: SmtpAuthentication::None,
            transport: SmtpTransport::Plain,
            verify_mode: None,
            ca_file: PathBuf::from("/etc/ssl/certs/ca-certificates.crt"),
        })),
        Url::parse("https://example.invalid/").unwrap(),
        SecretString::new("mail-secret".to_owned()),
    );
    let job = report_job(
        "moderator@example.invalid",
        "https://example.invalid/",
        42,
        "target@example.invalid",
        "reporter@example.invalid",
        7,
    );
    let expected_message_id = format!(
        "Message-ID: <{}@{}>",
        job.arguments()["message_id_local"].as_str().unwrap(),
        job.arguments()["message_id_domain"].as_str().unwrap()
    );
    let claimed = ClaimedJob {
        id: 2,
        lane: job.lane(),
        kind: job.kind().to_owned(),
        arguments: job.arguments().clone(),
        logical_key: job.logical_key_value().map(str::to_owned),
        run_at: Utc::now(),
        attempt: 1,
        max_attempts: 25,
        generation: 1,
        lease_owner: "mail-test".to_owned(),
        lease_expires_at: Utc::now() + Duration::minutes(1),
    };
    config
        .runtime()
        .unwrap()
        .unwrap()
        .send(&claimed)
        .await
        .unwrap();

    let message = tokio::time::timeout(StdDuration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let message = String::from_utf8(message).unwrap();
    assert!(message.contains("Subject: New report"), "{message:?}");
    assert!(
        message.contains("To: moderator@example.invalid"),
        "{message:?}"
    );
    assert!(message.contains(&expected_message_id), "{message:?}");
    assert!(!message.contains("rustodon-mail-2@"), "{message:?}");
    assert!(
        message.contains(
            "A new report was submitted for target@example.invalid by reporter@example.i=\r\nnvalid."
        ),
        "{message:?}"
    );
    assert!(message.contains("Report ID: 42"), "{message:?}");
}

async fn fixture_smtp_server(listener: TcpListener) -> Result<Vec<u8>, String> {
    let (socket, _) = listener.accept().await.map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    reader
        .get_mut()
        .write_all(b"220 fixture.test ESMTP\r\n")
        .await
        .map_err(|error| error.to_string())?;

    line.clear();
    reader
        .read_line(&mut line)
        .await
        .map_err(|error| error.to_string())?;
    if !line.to_ascii_uppercase().starts_with("EHLO ") {
        return Err(format!("expected EHLO, got {line:?}"));
    }
    reader
        .get_mut()
        .write_all(b"250-fixture.test\r\n250-8BITMIME\r\n250 OK\r\n")
        .await
        .map_err(|error| error.to_string())?;

    line.clear();
    reader
        .read_line(&mut line)
        .await
        .map_err(|error| error.to_string())?;
    if !line.to_ascii_uppercase().starts_with("MAIL FROM:") {
        return Err(format!("expected MAIL FROM, got {line:?}"));
    }
    reader
        .get_mut()
        .write_all(b"250 2.1.0 accepted\r\n")
        .await
        .map_err(|error| error.to_string())?;

    line.clear();
    reader
        .read_line(&mut line)
        .await
        .map_err(|error| error.to_string())?;
    if !line.to_ascii_uppercase().starts_with("RCPT TO:") {
        return Err(format!("expected RCPT TO, got {line:?}"));
    }
    reader
        .get_mut()
        .write_all(b"250 2.1.5 accepted\r\n")
        .await
        .map_err(|error| error.to_string())?;

    line.clear();
    reader
        .read_line(&mut line)
        .await
        .map_err(|error| error.to_string())?;
    if !line.eq_ignore_ascii_case("DATA\r\n") {
        return Err(format!("expected DATA, got {line:?}"));
    }
    reader
        .get_mut()
        .write_all(b"354 end with <CRLF>.<CRLF>\r\n")
        .await
        .map_err(|error| error.to_string())?;

    let mut message = Vec::new();
    loop {
        line.clear();
        reader
            .read_line(&mut line)
            .await
            .map_err(|error| error.to_string())?;
        if line == ".\r\n" {
            break;
        }
        message.extend_from_slice(line.as_bytes());
    }
    reader
        .get_mut()
        .write_all(b"250 2.0.0 queued\r\n")
        .await
        .map_err(|error| error.to_string())?;
    Ok(message)
}
