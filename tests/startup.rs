use rustodon::preflight::Diagnostic;
use rustodon::startup::{StartupReport, operational_failure};

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use reqwest::StatusCode;
use rustodon::jobs::{JobSpec, Lane, Queue};
use serde_json::json;
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};

#[test]
fn startup_reports_are_stable_and_do_not_render_internal_causes() {
    let report = StartupReport::from_diagnostics([
        Diagnostic::warning("STARTUP_WARNING", "warning", "hint"),
        operational_failure(),
    ]);
    assert!(!report.is_success());
    let rendered = report.to_string();
    assert!(rendered.starts_with("startup refused: 1 fatal, 1 warning"));
    assert!(rendered.contains("[FATAL STARTUP_OPERATIONAL_SCHEMA]"));
    for secret in [
        "postgresql://runtime:password@database/mastodon",
        "must-not-appear",
        "BEGIN PRIVATE KEY",
    ] {
        assert!(!rendered.contains(secret));
    }
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn web_probes_and_trusted_forwarding_are_operational()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime_url = std::env::var("RUSTODON_STARTUP_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_STARTUP_OWNER_DATABASE_URL")?;
    let port = unused_port()?;
    let mut child = command("web", &runtime_url, port).spawn()?;
    let base = format!("http://127.0.0.1:{port}");
    let result = async {
        let client = reqwest::Client::builder().no_proxy().build()?;
        wait_for_status(&client, &format!("{base}/health"), StatusCode::OK).await?;
        assert_eq!(
            client.get(format!("{base}/ready")).send().await?.status(),
            StatusCode::OK
        );
        assert_eq!(
            client
                .get(format!("{base}/api/v2/instance"))
                .header("host", "attacker.invalid")
                .header("x-forwarded-host", "fixture-v4-6-5.rustodon.invalid")
                .header("x-forwarded-proto", "https")
                .header("x-forwarded-for", "198.51.100.9")
                .send()
                .await?
                .status(),
            StatusCode::OK
        );

        let mut owner = PgConnection::connect(&owner_url).await?;
        sqlx::query("REVOKE SELECT ON public.accounts FROM rustodon_worker_runtime")
            .execute(&mut owner)
            .await?;
        wait_for_status(
            &client,
            &format!("{base}/ready"),
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await?;
        sqlx::query("GRANT SELECT ON public.accounts TO rustodon_worker_runtime")
            .execute(&mut owner)
            .await?;
        wait_for_status(&client, &format!("{base}/ready"), StatusCode::OK).await?;

        sqlx::raw_sql(
            "ALTER ROLE rustodon_worker_runtime NOLOGIN; \
             SELECT pg_catalog.pg_terminate_backend(pid) FROM pg_catalog.pg_stat_activity \
             WHERE usename = 'rustodon_worker_runtime' AND pid <> pg_catalog.pg_backend_pid()",
        )
        .execute(&mut owner)
        .await?;
        let degraded = async {
            wait_for_status(
                &client,
                &format!("{base}/ready"),
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .await?;
            Ok::<_, Box<dyn std::error::Error>>(
                client.get(format!("{base}/health")).send().await?.status(),
            )
        }
        .await;
        sqlx::query("ALTER ROLE rustodon_worker_runtime LOGIN")
            .execute(&mut owner)
            .await?;
        assert_eq!(degraded?, StatusCode::OK);
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    terminate(&mut child)?;
    result
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn fatal_startup_binds_no_web_service_and_claims_no_worker_work()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime_url = std::env::var("RUSTODON_STARTUP_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_STARTUP_OWNER_DATABASE_URL")?;
    let port = unused_port()?;
    let web = command("web", &runtime_url, port)
        .env("S3_ENABLED", "true")
        .output()?;
    assert_refused(&web, &runtime_url);
    assert!(
        TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(100)
        )
        .is_err()
    );

    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::raw_sql(
        "TRUNCATE rustodon.durable_jobs, rustodon.outbox_events, rustodon.heartbeats \
         RESTART IDENTITY",
    )
    .execute(&mut owner)
    .await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&runtime_url)
        .await?;
    let queue = Queue::new(pool.clone());
    let job_id = queue
        .enqueue(&JobSpec::new(
            Lane::Maintenance,
            "rustodon.maintenance.prune",
            json!({"startup": false}),
        ))
        .await?;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
         VALUES ('rustodon.maintenance.prune', 'startup-pending', \
           jsonb_build_object('lane', 'maintenance', 'arguments', '{}'::jsonb, \
             'run_at', clock_timestamp(), 'max_attempts', 2))",
    )
    .execute(&pool)
    .await?;
    let worker = command("worker", &runtime_url, unused_port()?)
        .env("S3_ENABLED", "true")
        .output()?;
    assert_refused(&worker, &runtime_url);
    let row = sqlx::query_as::<_, (i32, Option<String>)>(
        "SELECT attempts, lease_owner FROM rustodon.durable_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(row, (0, None));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rustodon.heartbeats")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events WHERE dispatched_at IS NOT NULL",
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    Ok(())
}

fn command(mode: &str, database_url: &str, port: u16) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustodon"));
    command
        .arg(mode)
        .env_clear()
        .env("LOCAL_DOMAIN", "fixture-v4-6-5.rustodon.invalid")
        .env("WEB_DOMAIN", "fixture-v4-6-5.rustodon.invalid")
        .env("DATABASE_URL", database_url)
        .env(
            "PAPERCLIP_ROOT_PATH",
            std::env::var("RUSTODON_STARTUP_MEDIA_ROOT").unwrap(),
        )
        .env("PAPERCLIP_ROOT_URL", "/system")
        .env("TRUSTED_PROXY_IP", "127.0.0.1/32")
        .env("BIND", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("WORKER_LANES", "maintenance")
        .env("WORKER_HEARTBEAT_SECONDS", "1")
        .env("WORKER_POLL_MILLISECONDS", "25")
        .env("SECRET_KEY_BASE", "must-not-appear-secret")
        .env(
            "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY",
            "11111111111111111111111111111111",
        )
        .env(
            "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT",
            "22222222222222222222222222222222",
        )
        .env(
            "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY",
            "33333333333333333333333333333333",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn unused_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.local_addr().map(|address| address.port())
}

async fn wait_for_status(
    client: &reqwest::Client,
    url: &str,
    expected: StatusCode,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..100 {
        if client
            .get(url)
            .send()
            .await
            .is_ok_and(|response| response.status() == expected)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("{url} did not return {expected}").into())
}

fn terminate(child: &mut Child) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()?;
    assert!(status.success());
    assert!(child.wait()?.success());
    Ok(())
}

fn assert_refused(output: &Output, database_url: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PF_CONFIG_OBJECT_STORAGE_S3"));
    assert!(!stderr.contains("must-not-appear-secret"));
    assert!(!stderr.contains(database_url));
}
