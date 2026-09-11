use std::error::Error;
use std::net::{IpAddr, SocketAddr};

use http::{HeaderMap, HeaderValue, StatusCode};
use ipnetwork::IpNetwork;
use reqwest::Method;
use rustodon::mastodon::{Repository, WriteRepository, password_verification_count};
use rustodon::secret::SecretString;
use rustodon::web::{WebState, router};
use sqlx::{Connection, PgConnection};
use tokio::sync::oneshot;
use url::Url;

use super::comparison::CapturedResponse;
use super::harness::{RequestSpec, send_single};
use super::reauth::fixture_browser_session;
use super::safety::DifferentialConfig;
use super::writes::{BrowserFormState, load_browser_form};

const ROUTES: [&str; 6] = [
    "/settings/security",
    "/settings/two_factor_authentication_methods/disable",
    "/settings/two_factor_authentication/recovery_codes",
    "/settings/otp_authentication",
    "/settings/two_factor_authentication/confirmation",
    "/settings/delete",
];

pub async fn run(config: DifferentialConfig) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mut targets = Vec::new();
    let mut servers = Vec::new();
    for _ in 0..2 {
        let writer =
            WriteRepository::connect(config.rust_write_database.as_ref().unwrap().url()).await?;
        let state = WebState::new(
            Repository::connect(config.rust_database.url()).await?,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            crate::fixture_instance_runtime(),
            vec!["127.0.0.0/8".parse()?],
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer)
        .with_csrf_signing_secret(&SecretString::new("fixture-reauth-csrf-secret".to_owned()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        targets.push(Url::parse(&format!("http://{}", listener.local_addr()?))?);
        let (shutdown, receive) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router(state).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = receive.await;
            })
            .await
        });
        servers.push((shutdown, server));
    }
    let result = check_limits(&config, &targets).await;
    for (shutdown, server) in servers {
        let _ = shutdown.send(());
        server.await??;
    }
    result
}

async fn browser(
    config: &DifferentialConfig,
    target: &Url,
    username: &str,
) -> Result<BrowserFormState, Box<dyn Error>> {
    let url = config.rust_write_database.as_ref().unwrap().url();
    let writer = WriteRepository::connect(url).await?;
    let user = writer
        .create_local_user(
            &format!("{username}@fixture.invalid"),
            username,
            "fixture-password",
        )
        .await?;
    let session = fixture_browser_session(
        url,
        user.user_id,
        IpNetwork::from("192.0.2.99".parse::<IpAddr>()?),
        "reauth-limit-test",
    )
    .await?;
    let mut browser = BrowserFormState::new(Some(&session));
    load_browser_form(
        target,
        "/settings/security",
        &mut browser,
        "csrf_token",
        "reauth limits",
    )
    .await?;
    Ok(browser)
}

async fn challenge(
    target: &Url,
    browser: &BrowserFormState,
    path: &str,
    ip: &str,
    password: &str,
) -> Result<CapturedResponse, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "host",
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert("accept", HeaderValue::from_static("text/html"));
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert("cookie", HeaderValue::from_str(&browser.cookie_header())?);
    headers.insert("x-forwarded-for", HeaderValue::from_str(ip)?);
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("csrf_token", browser.csrf_token()?)
        .append_pair("current_password", password)
        .append_pair("password", password)
        .append_pair("password_confirmation", password)
        .append_pair("otp_secret", "JBSWY3DPEHPK3PXP")
        .append_pair("otp_attempt", "000000")
        .finish();
    Ok(send_single(
        target,
        &RequestSpec::new(Method::POST, path, None, headers, body.into_bytes())?,
        "reauth limits",
    )
    .await?)
}

#[allow(clippy::too_many_lines)]
async fn check_limits(config: &DifferentialConfig, targets: &[Url]) -> Result<(), Box<dyn Error>> {
    let account = browser(config, &targets[0], "limitaccount").await?;
    // Switching endpoints, processes/pools, and IP addresses must not replenish the user budget.
    for attempt in 0..10 {
        let before = password_verification_count();
        let response = challenge(
            &targets[attempt % 2],
            &account,
            ROUTES[attempt % ROUTES.len()],
            &format!("192.0.2.{}", 10 + attempt),
            "wrong-fixture-password",
        )
        .await?;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY.as_u16(),
            "attempt {attempt}"
        );
        assert_eq!(password_verification_count(), before + 1);
    }
    for path in ROUTES {
        for password in ["wrong-fixture-password", "fixture-password"] {
            let before = password_verification_count();
            let response =
                challenge(&targets[1], &account, path, "198.51.100.80", password).await?;
            assert_eq!(
                response.status,
                StatusCode::TOO_MANY_REQUESTS.as_u16(),
                "user budget: {path}"
            );
            assert!(response.headers.contains_key("retry-after"));
            assert_eq!(
                password_verification_count(),
                before,
                "limited request reached bcrypt: {path}"
            );
        }
    }
    eprintln!(
        "user budget: 10 challenges shared across six routes/two instances/IP changes; all six blocked before bcrypt"
    );

    let legitimate = browser(config, &targets[1], "limitlegitimate").await?;
    let before = password_verification_count();
    let response = challenge(
        &targets[1],
        &legitimate,
        ROUTES[3],
        "198.51.100.81",
        "fixture-password",
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK.as_u16());
    assert_eq!(password_verification_count(), before + 1);

    // A separate IP budget prevents rotating accounts from bypassing the cap.
    let mut last_account = None;
    for (index, count) in [10, 10, 5].into_iter().enumerate() {
        let account = browser(config, &targets[0], &format!("limitip{index}")).await?;
        for attempt in 0..count {
            let response = challenge(
                &targets[attempt % 2],
                &account,
                ROUTES[attempt % ROUTES.len()],
                "203.0.113.200",
                "wrong-fixture-password",
            )
            .await?;
            assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY.as_u16());
        }
        last_account = Some(account);
    }
    let account = last_account.unwrap();
    let before = password_verification_count();
    let response = challenge(
        &targets[1],
        &account,
        ROUTES[1],
        "203.0.113.200",
        "fixture-password",
    )
    .await?;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS.as_u16());
    assert_eq!(password_verification_count(), before);
    // Expire only fixture windows deterministically; no wall-clock sleep.
    let mut connection =
        PgConnection::connect(config.rust_write_database.as_ref().unwrap().url()).await?;
    sqlx::query("UPDATE rustodon.rate_limit_windows SET expires_at = clock_timestamp() - INTERVAL '1 second' WHERE window_key LIKE 'browser_reauthentication:%'").execute(&mut connection).await?;
    let response = challenge(
        &targets[0],
        &account,
        ROUTES[3],
        "203.0.113.200",
        "fixture-password",
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK.as_u16());
    assert_eq!(password_verification_count(), before + 1);
    eprintln!(
        "IP budget: 25 challenges shared across three users/two instances; blocked before bcrypt; expiry and legitimate reauthentication work"
    );
    Ok(())
}
