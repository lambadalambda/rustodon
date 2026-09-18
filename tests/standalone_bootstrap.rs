#![cfg(feature = "test-support")]

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, Rgb, RgbImage};
use rustodon::bootstrap::{BootstrapOutcome, BootstrapRequest, bootstrap_instance};
use rustodon::jobs::Queue;
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository, verify_password};
use rustodon::web::{WebState, router};
use rustodon::{operational_schema, preflight};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool};
use url::Url;

fn required_environment(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for this ignored test"))
}

fn runtime(domain: &str) -> InstanceRuntimeConfig {
    InstanceRuntimeConfig {
        domain: domain.to_owned(),
        version: "4.6.5".to_owned(),
        source_url: String::new(),
        streaming_api: String::new(),
        vapid_public_key: None,
        thumbnail_url: String::new(),
        thumbnail_description: String::new(),
        thumbnail_blurhash: None,
        thumbnail_versions: None,
        icons: Vec::new(),
        languages: vec!["en".to_owned()],
        active_month: 0,
        active_halfyear: 0,
        translation_enabled: false,
        limited_federation: false,
        single_user_mode: false,
        terms_of_service_url: None,
        sso_signup_url: None,
        wrapstodon: None,
    }
}

async fn success_json(
    response: reqwest::Response,
    operation: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let status = response.status();
    let body = response.bytes().await?;
    assert!(
        status.is_success(),
        "{operation} returned {status}: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(serde_json::from_slice(&body)?)
}

fn response_cookie(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .find(|cookie| cookie.starts_with(&format!("{name}=")))
        .map(ToOwned::to_owned)
}

async fn login_first_owner(
    client: &reqwest::Client,
    base: &str,
    domain: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let sign_in_page = client
        .get(format!("{base}/auth/sign_in"))
        .header("host", domain)
        .header("x-forwarded-proto", "https")
        .header("accept", "text/html")
        .send()
        .await?;
    assert!(sign_in_page.status().is_success());
    let csrf_cookie = response_cookie(&sign_in_page, "__Host-csrf_token")
        .ok_or("sign-in page did not set its CSRF cookie")?;
    let csrf_token = csrf_cookie
        .split_once('=')
        .map(|(_, value)| value)
        .ok_or("sign-in CSRF cookie is malformed")?;
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("csrf_token", csrf_token)
        .append_pair("user[email]", "alice@bootstrap.invalid")
        .append_pair("user[password]", "bootstrap-test-password")
        .finish();
    let login = client
        .post(format!("{base}/auth/sign_in"))
        .header("host", domain)
        .header("x-forwarded-proto", "https")
        .header("accept", "text/html")
        .header("cookie", &csrf_cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    assert_eq!(login.status(), reqwest::StatusCode::FOUND);
    assert_eq!(
        login.headers().get(reqwest::header::LOCATION),
        Some(&reqwest::header::HeaderValue::from_static("/"))
    );
    let session_cookie = response_cookie(&login, "_mastodon_session")
        .ok_or("successful sign-in did not set a browser session")?;
    let shell = client
        .get(format!("{base}/"))
        .header("host", domain)
        .header("x-forwarded-proto", "https")
        .header("accept", "text/html")
        .header("cookie", session_cookie)
        .send()
        .await?;
    assert!(shell.status().is_success());
    Ok(())
}

async fn upload_test_jpeg(
    client: &reqwest::Client,
    base: &str,
    domain: &str,
    token: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut jpeg = Vec::new();
    JpegEncoder::new(&mut Cursor::new(&mut jpeg)).encode_image(&DynamicImage::ImageRgb8(
        RgbImage::from_pixel(16, 12, Rgb([20, 100, 180])),
    ))?;
    let mut multipart = b"--rustodon-bootstrap-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"bootstrap.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
    multipart.extend_from_slice(&jpeg);
    multipart.extend_from_slice(b"\r\n--rustodon-bootstrap-boundary--\r\n");
    let response = client
        .post(format!("{base}/api/v2/media"))
        .header("host", domain)
        .header("x-forwarded-proto", "https")
        .header("authorization", format!("Bearer {token}"))
        .header(
            "content-type",
            "multipart/form-data; boundary=rustodon-bootstrap-boundary",
        )
        .body(multipart)
        .send()
        .await?;
    let media = success_json(response, "media upload").await?;
    Ok(media["id"]
        .as_str()
        .ok_or("media response has no id")?
        .to_owned())
}

async fn standalone_http_smoke(
    runtime_database_url: &str,
    writer_database_url: &str,
    media_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    const DOMAIN: &str = "standalone-bootstrap.rustodon.invalid";
    const TOKEN: &str = "standalone-bootstrap-test-token";
    let writer = WriteRepository::connect(writer_database_url).await?;
    let (user_id, password_hash) = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, encrypted_password FROM public.users WHERE email = 'alice@bootstrap.invalid'",
    )
    .fetch_one(writer.pool())
    .await?;
    assert!(verify_password("bootstrap-test-password", &password_hash));
    sqlx::query(
        "INSERT INTO public.oauth_access_tokens \
           (resource_owner_id, token, scopes, created_at) \
         VALUES ($1, $2, 'read write', clock_timestamp())",
    )
    .bind(user_id)
    .bind(TOKEN)
    .execute(writer.pool())
    .await?;

    let runtime_pool = PgPool::connect(runtime_database_url).await?;
    let state = WebState::new(
        Repository::connect(runtime_database_url).await?,
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        media_root,
        runtime(DOMAIN),
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?
    .with_write_repository(writer)
    .with_queue(Queue::new(runtime_pool));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    login_first_owner(&client, &base, DOMAIN).await?;
    let media_id = upload_test_jpeg(&client, &base, DOMAIN, TOKEN).await?;

    let status_response = client
        .post(format!("{base}/api/v1/statuses"))
        .header("host", DOMAIN)
        .header("x-forwarded-proto", "https")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(
            json!({
                "status": "Standalone bootstrap smoke",
                "visibility": "public",
                "media_ids": [media_id]
            })
            .to_string(),
        )
        .send()
        .await?;
    let status = success_json(status_response, "status creation").await?;
    let status_id = status["id"].as_str().ok_or("status response has no id")?;

    let webfinger = client
        .get(format!(
            "{base}/.well-known/webfinger?resource=acct:alice@{DOMAIN}"
        ))
        .header("host", DOMAIN)
        .header("x-forwarded-proto", "https")
        .send()
        .await?;
    let webfinger = success_json(webfinger, "WebFinger lookup").await?;
    assert_eq!(webfinger["subject"], format!("acct:alice@{DOMAIN}"));

    let actor = client
        .get(format!("{base}/users/alice"))
        .header("host", DOMAIN)
        .header("x-forwarded-proto", "https")
        .header("accept", "application/activity+json")
        .send()
        .await?;
    let actor = success_json(actor, "ActivityPub actor lookup").await?;
    assert_eq!(actor["preferredUsername"], "alice");
    assert!(
        actor["id"]
            .as_str()
            .is_some_and(|id| id.starts_with(&format!("https://{DOMAIN}/ap/users/")))
    );

    let public_status = client
        .get(format!("{base}/api/v1/statuses/{status_id}"))
        .header("host", DOMAIN)
        .header("x-forwarded-proto", "https")
        .send()
        .await?;
    let public_status = success_json(public_status, "public status lookup").await?;
    assert_eq!(public_status["id"], status_id);
    server.abort();
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL 14 database and empty media root"]
#[allow(clippy::too_many_lines, clippy::manual_assert_eq)]
async fn standalone_bootstrap_installs_and_verifies_exact_baseline() {
    let database_url = required_environment("RUSTODON_BOOTSTRAP_DATABASE_URL");
    let cluster_admin_database_url =
        required_environment("RUSTODON_BOOTSTRAP_CLUSTER_ADMIN_DATABASE_URL");
    let runtime_database_url = required_environment("RUSTODON_BOOTSTRAP_RUNTIME_DATABASE_URL");
    let writer_database_url = required_environment("RUSTODON_BOOTSTRAP_WRITER_DATABASE_URL");
    let runtime_role = required_environment("RUSTODON_BOOTSTRAP_RUNTIME_ROLE");
    let writer_role = required_environment("RUSTODON_BOOTSTRAP_WRITER_ROLE");
    let media_root = PathBuf::from(required_environment("RUSTODON_BOOTSTRAP_MEDIA_ROOT"));
    let mut connection = PgConnection::connect(&database_url)
        .await
        .expect("connect installer database");
    let mut cluster_admin_connection = PgConnection::connect(&cluster_admin_database_url)
        .await
        .expect("connect fixture cluster administrator");
    let request = BootstrapRequest {
        admin_username: "alice",
        admin_email: "alice@bootstrap.invalid",
        admin_password: "bootstrap-test-password",
        site_title: "Bootstrap Test",
        runtime_role: &runtime_role,
        writer_role: &writer_role,
        media_root: &media_root,
    };

    let unexpected_media = media_root.join("unexpected-bootstrap-media");
    std::fs::write(&unexpected_media, b"not empty").expect("create conflicting media file");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    std::fs::remove_file(unexpected_media).expect("remove conflicting media file");

    sqlx::query("CREATE SCHEMA unexpected_bootstrap_state")
        .execute(&mut connection)
        .await
        .expect("create conflicting schema");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    sqlx::query("DROP SCHEMA unexpected_bootstrap_state")
        .execute(&mut connection)
        .await
        .expect("remove conflicting schema");

    let quoted_runtime = sqlx::query_scalar::<_, String>("SELECT pg_catalog.quote_ident($1)")
        .bind(&runtime_role)
        .fetch_one(&mut connection)
        .await
        .expect("quote runtime role");
    let quoted_database =
        sqlx::query_scalar::<_, String>("SELECT pg_catalog.quote_ident(current_database())")
            .fetch_one(&mut connection)
            .await
            .expect("quote database");
    sqlx::query(&format!(
        "ALTER ROLE {quoted_runtime} IN DATABASE {quoted_database} SET default_transaction_read_only = on"
    ))
    .execute(&mut cluster_admin_connection)
    .await
    .expect("set conflicting database role setting");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    sqlx::query(&format!(
        "ALTER ROLE {quoted_runtime} IN DATABASE {quoted_database} RESET default_transaction_read_only"
    ))
    .execute(&mut cluster_admin_connection)
    .await
    .expect("reset conflicting database role setting");

    sqlx::query(&format!(
        "GRANT CREATE ON SCHEMA public TO {quoted_runtime}"
    ))
    .execute(&mut connection)
    .await
    .expect("grant conflicting public privilege");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    sqlx::query(&format!(
        "REVOKE CREATE ON SCHEMA public FROM {quoted_runtime}"
    ))
    .execute(&mut connection)
    .await
    .expect("remove conflicting public privilege");

    assert_eq!(
        bootstrap_instance(&mut connection, &request)
            .await
            .expect("install standalone baseline"),
        BootstrapOutcome::Installed
    );
    let identity_before = sqlx::query_as::<_, (String, String, String)>(
        "SELECT actor.public_key, admin.public_key, user_record.encrypted_password \
         FROM public.accounts actor CROSS JOIN public.accounts admin \
         JOIN public.users user_record ON user_record.account_id = admin.id \
         WHERE actor.id = -99 AND admin.id <> -99",
    )
    .fetch_one(&mut connection)
    .await
    .expect("read generated identity before verification rerun");
    assert_ne!(identity_before.0, identity_before.1);
    let migration_fingerprint = sqlx::query_as::<_, (i64, String)>(
        "SELECT count(*), max(version)::text FROM public.schema_migrations",
    )
    .fetch_one(&mut connection)
    .await
    .expect("read migration fingerprint");
    assert_eq!(migration_fingerprint, (588, "20260611150940".to_owned()));
    assert_eq!(
        bootstrap_instance(&mut connection, &request)
            .await
            .expect("verify standalone baseline"),
        BootstrapOutcome::Verified
    );
    let identity_after = sqlx::query_as::<_, (String, String, String)>(
        "SELECT actor.public_key, admin.public_key, user_record.encrypted_password \
         FROM public.accounts actor CROSS JOIN public.accounts admin \
         JOIN public.users user_record ON user_record.account_id = admin.id \
         WHERE actor.id = -99 AND admin.id <> -99",
    )
    .fetch_one(&mut connection)
    .await
    .expect("read generated identity after verification rerun");
    assert!(identity_before == identity_after);

    let counts = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM public.accounts), \
                (SELECT count(*) FROM public.users), \
                (SELECT count(*) FROM public.statuses), \
                (SELECT count(*) FROM public.media_attachments)",
    )
    .fetch_one(&mut connection)
    .await
    .expect("read baseline counts");
    assert_eq!(counts, (2, 1, 0, 0));
    let admin = sqlx::query_as::<_, (bool, bool, bool, String)>(
        "SELECT user_record.approved, NOT user_record.disabled, \
                user_record.confirmed_at IS NOT NULL, role.name::text \
         FROM public.users user_record JOIN public.user_roles role ON role.id = user_record.role_id",
    )
    .fetch_one(&mut connection)
    .await
    .expect("read first owner");
    assert_eq!(admin, (true, true, true, "Owner".to_owned()));

    let mut runtime_connection = PgConnection::connect(&runtime_database_url)
        .await
        .expect("connect runtime database");
    sqlx::query("SELECT pg_catalog.set_config('rustodon.writer_role', $1, false)")
        .bind(&writer_role)
        .execute(&mut runtime_connection)
        .await
        .expect("configure expected writer role");
    operational_schema::validate(&mut runtime_connection)
        .await
        .expect("runtime connection passes operational validation");
    let mut writer_connection = PgConnection::connect(&writer_database_url)
        .await
        .expect("connect writer database");
    preflight::validate_writer_connection(&mut writer_connection)
        .await
        .expect("writer connection passes exact privilege validation");

    sqlx::query("CREATE SCHEMA unexpected_completed_state")
        .execute(&mut connection)
        .await
        .expect("introduce completed-schema drift");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    sqlx::query("DROP SCHEMA unexpected_completed_state")
        .execute(&mut connection)
        .await
        .expect("remove completed-schema drift");

    sqlx::query(
        "CREATE RULE unexpected_tag_insert AS \
         ON INSERT TO public.tags DO INSTEAD NOTHING",
    )
    .execute(&mut connection)
    .await
    .expect("introduce relation-rule drift");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    sqlx::query("DROP RULE unexpected_tag_insert ON public.tags")
        .execute(&mut connection)
        .await
        .expect("remove relation-rule drift");

    sqlx::query(
        "INSERT INTO public.tags (name, created_at, updated_at) \
         VALUES ('unexpected', clock_timestamp(), clock_timestamp())",
    )
    .execute(&mut connection)
    .await
    .expect("introduce application-row drift");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    sqlx::query("DELETE FROM public.tags WHERE name = 'unexpected'")
        .execute(&mut connection)
        .await
        .expect("remove application-row drift");

    sqlx::query("UPDATE public.username_blocks SET username = 'changed' WHERE username = 'abuse'")
        .execute(&mut connection)
        .await
        .expect("introduce reserved-name drift");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    sqlx::query("UPDATE public.username_blocks SET username = 'abuse' WHERE username = 'changed'")
        .execute(&mut connection)
        .await
        .expect("remove reserved-name drift");

    drop(writer_connection);
    drop(runtime_connection);
    drop(cluster_admin_connection);
    drop(connection);
    standalone_http_smoke(&runtime_database_url, &writer_database_url, &media_root)
        .await
        .expect("standalone HTTP smoke");
}
