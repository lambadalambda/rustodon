use rustodon::preflight::Diagnostic;
use rustodon::startup::{StartupReport, operational_failure};

use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use rustodon::jobs::{JobSpec, Lane, Queue};
use serde_json::json;
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};

const WRITER_PRIVILEGE_MUTATIONS: &[(&str, &str)] = &[
    (
        "REVOKE INSERT ON public.polls FROM rustodon_differential_writer",
        "GRANT INSERT ON public.polls TO rustodon_differential_writer",
    ),
    (
        "REVOKE UPDATE ON public.polls FROM rustodon_differential_writer",
        "GRANT UPDATE ON public.polls TO rustodon_differential_writer",
    ),
    (
        "REVOKE DELETE ON public.polls FROM rustodon_differential_writer",
        "GRANT DELETE ON public.polls TO rustodon_differential_writer",
    ),
    (
        "REVOKE INSERT ON public.poll_votes FROM rustodon_differential_writer",
        "GRANT INSERT ON public.poll_votes TO rustodon_differential_writer",
    ),
    (
        "REVOKE DELETE ON public.poll_votes FROM rustodon_differential_writer",
        "GRANT DELETE ON public.poll_votes TO rustodon_differential_writer",
    ),
    (
        "REVOKE USAGE ON SEQUENCE public.polls_id_seq FROM rustodon_differential_writer",
        "GRANT USAGE ON SEQUENCE public.polls_id_seq TO rustodon_differential_writer",
    ),
    (
        "REVOKE USAGE ON SEQUENCE public.poll_votes_id_seq FROM rustodon_differential_writer",
        "GRANT USAGE ON SEQUENCE public.poll_votes_id_seq TO rustodon_differential_writer",
    ),
    (
        "GRANT SELECT ON SEQUENCE public.polls_id_seq TO rustodon_differential_writer",
        "REVOKE SELECT ON SEQUENCE public.polls_id_seq FROM rustodon_differential_writer",
    ),
    (
        "GRANT UPDATE ON SEQUENCE public.polls_id_seq TO rustodon_differential_writer",
        "REVOKE UPDATE ON SEQUENCE public.polls_id_seq FROM rustodon_differential_writer",
    ),
    (
        "GRANT SELECT ON SEQUENCE public.poll_votes_id_seq TO rustodon_differential_writer",
        "REVOKE SELECT ON SEQUENCE public.poll_votes_id_seq FROM rustodon_differential_writer",
    ),
    (
        "GRANT UPDATE ON SEQUENCE public.poll_votes_id_seq TO rustodon_differential_writer",
        "REVOKE UPDATE ON SEQUENCE public.poll_votes_id_seq FROM rustodon_differential_writer",
    ),
    (
        "REVOKE SELECT ON public.web_settings FROM rustodon_differential_writer",
        "GRANT SELECT ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "REVOKE INSERT (user_id) ON public.web_settings FROM rustodon_differential_writer",
        "GRANT INSERT (user_id) ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "REVOKE INSERT (data) ON public.web_settings FROM rustodon_differential_writer",
        "GRANT INSERT (data) ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "REVOKE INSERT (created_at) ON public.web_settings FROM rustodon_differential_writer",
        "GRANT INSERT (created_at) ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "REVOKE INSERT (updated_at) ON public.web_settings FROM rustodon_differential_writer",
        "GRANT INSERT (updated_at) ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "REVOKE UPDATE (data) ON public.web_settings FROM rustodon_differential_writer",
        "GRANT UPDATE (data) ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "REVOKE UPDATE (updated_at) ON public.web_settings FROM rustodon_differential_writer",
        "GRANT UPDATE (updated_at) ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "GRANT DELETE ON public.web_settings TO rustodon_differential_writer",
        "REVOKE DELETE ON public.web_settings FROM rustodon_differential_writer",
    ),
    (
        "GRANT UPDATE (user_id) ON public.web_settings TO rustodon_differential_writer",
        "REVOKE UPDATE (user_id) ON public.web_settings FROM rustodon_differential_writer",
    ),
    (
        "GRANT UPDATE (created_at) ON public.web_settings TO rustodon_differential_writer",
        "REVOKE UPDATE (created_at) ON public.web_settings FROM rustodon_differential_writer",
    ),
    (
        "GRANT INSERT (id) ON public.web_settings TO rustodon_differential_writer",
        "REVOKE INSERT (id) ON public.web_settings FROM rustodon_differential_writer",
    ),
    (
        "GRANT INSERT ON public.web_settings TO rustodon_differential_writer",
        "REVOKE INSERT ON public.web_settings FROM rustodon_differential_writer; GRANT INSERT (user_id, data, created_at, updated_at) ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "GRANT UPDATE ON public.web_settings TO rustodon_differential_writer",
        "REVOKE UPDATE ON public.web_settings FROM rustodon_differential_writer; GRANT UPDATE (data, updated_at) ON public.web_settings TO rustodon_differential_writer",
    ),
    (
        "REVOKE USAGE ON SEQUENCE public.web_settings_id_seq FROM rustodon_differential_writer",
        "GRANT USAGE ON SEQUENCE public.web_settings_id_seq TO rustodon_differential_writer",
    ),
    (
        "GRANT SELECT ON SEQUENCE public.web_settings_id_seq TO rustodon_differential_writer",
        "REVOKE SELECT ON SEQUENCE public.web_settings_id_seq FROM rustodon_differential_writer",
    ),
    (
        "GRANT UPDATE ON SEQUENCE public.web_settings_id_seq TO rustodon_differential_writer",
        "REVOKE UPDATE ON SEQUENCE public.web_settings_id_seq FROM rustodon_differential_writer",
    ),
    (
        "ALTER ROLE rustodon_differential_writer SUPERUSER",
        "ALTER ROLE rustodon_differential_writer NOSUPERUSER",
    ),
    (
        "ALTER ROLE rustodon_differential_writer CREATEROLE",
        "ALTER ROLE rustodon_differential_writer NOCREATEROLE",
    ),
    (
        "ALTER ROLE rustodon_differential_writer CREATEDB",
        "ALTER ROLE rustodon_differential_writer NOCREATEDB",
    ),
    (
        "ALTER ROLE rustodon_differential_writer REPLICATION",
        "ALTER ROLE rustodon_differential_writer NOREPLICATION",
    ),
    (
        "ALTER ROLE rustodon_differential_writer BYPASSRLS",
        "ALTER ROLE rustodon_differential_writer NOBYPASSRLS",
    ),
    (
        "GRANT CREATE ON DATABASE rustodon_mastodon_v4_6_5_fixture TO rustodon_differential_writer",
        "REVOKE CREATE ON DATABASE rustodon_mastodon_v4_6_5_fixture FROM rustodon_differential_writer",
    ),
    (
        "GRANT CONNECT ON DATABASE rustodon_mastodon_v4_6_5_fixture TO rustodon_differential_writer WITH GRANT OPTION",
        "REVOKE GRANT OPTION FOR CONNECT ON DATABASE rustodon_mastodon_v4_6_5_fixture FROM rustodon_differential_writer",
    ),
    (
        "GRANT CREATE ON SCHEMA public TO rustodon_differential_writer",
        "REVOKE CREATE ON SCHEMA public FROM rustodon_differential_writer",
    ),
    (
        "GRANT USAGE ON SCHEMA public TO rustodon_differential_writer WITH GRANT OPTION",
        "REVOKE GRANT OPTION FOR USAGE ON SCHEMA public FROM rustodon_differential_writer",
    ),
    (
        "GRANT rustodon_writer_group TO rustodon_differential_writer",
        "REVOKE rustodon_writer_group FROM rustodon_differential_writer",
    ),
    (
        "ALTER DEFAULT PRIVILEGES FOR ROLE rustodon_differential_writer IN SCHEMA public GRANT INSERT ON TABLES TO rustodon_differential_writer",
        "ALTER DEFAULT PRIVILEGES FOR ROLE rustodon_differential_writer IN SCHEMA public REVOKE INSERT ON TABLES FROM rustodon_differential_writer",
    ),
    (
        "ALTER DEFAULT PRIVILEGES FOR ROLE rustodon_differential_writer IN SCHEMA public GRANT SELECT ON TABLES TO rustodon_differential_writer",
        "ALTER DEFAULT PRIVILEGES FOR ROLE rustodon_differential_writer IN SCHEMA public REVOKE SELECT ON TABLES FROM rustodon_differential_writer",
    ),
    (
        "ALTER DEFAULT PRIVILEGES FOR ROLE rustodon_differential_writer GRANT USAGE, CREATE ON SCHEMAS TO PUBLIC",
        "ALTER DEFAULT PRIVILEGES FOR ROLE rustodon_differential_writer REVOKE ALL ON SCHEMAS FROM PUBLIC",
    ),
    (
        "ALTER FUNCTION public.rustodon_refresh_instances() SECURITY INVOKER",
        "ALTER FUNCTION public.rustodon_refresh_instances() SECURITY DEFINER",
    ),
    (
        r"CREATE OR REPLACE FUNCTION public.rustodon_refresh_instances()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
  PERFORM 1;
END
$$",
        r"CREATE OR REPLACE FUNCTION public.rustodon_refresh_instances()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
  REFRESH MATERIALIZED VIEW CONCURRENTLY public.instances;
END
$$;
GRANT EXECUTE ON FUNCTION public.rustodon_refresh_instances() TO rustodon_differential_writer",
    ),
    (
        "GRANT INSERT ON TABLE public.quotes TO rustodon_differential_writer",
        "REVOKE INSERT ON TABLE public.quotes FROM rustodon_differential_writer",
    ),
    (
        "GRANT INSERT (id) ON TABLE public.quotes TO rustodon_differential_writer",
        "REVOKE INSERT (id) ON TABLE public.quotes FROM rustodon_differential_writer",
    ),
    (
        "GRANT TRUNCATE ON TABLE public.quotes TO rustodon_differential_writer",
        "REVOKE TRUNCATE ON TABLE public.quotes FROM rustodon_differential_writer",
    ),
    (
        "GRANT REFERENCES ON TABLE public.quotes TO rustodon_differential_writer",
        "REVOKE REFERENCES ON TABLE public.quotes FROM rustodon_differential_writer",
    ),
    (
        "GRANT TRIGGER ON TABLE public.quotes TO rustodon_differential_writer",
        "REVOKE TRIGGER ON TABLE public.quotes FROM rustodon_differential_writer",
    ),
    (
        "GRANT UPDATE ON TABLE public.mentions TO rustodon_differential_writer",
        "REVOKE UPDATE ON TABLE public.mentions FROM rustodon_differential_writer; \
         GRANT UPDATE (silent, updated_at) ON TABLE public.mentions TO rustodon_differential_writer",
    ),
    (
        "GRANT SELECT ON TABLE public.account_statuses_cleanup_policies TO rustodon_differential_writer",
        "REVOKE SELECT ON TABLE public.account_statuses_cleanup_policies FROM rustodon_differential_writer",
    ),
    (
        "GRANT SELECT (var) ON TABLE public.settings TO rustodon_differential_writer",
        "REVOKE SELECT (var) ON TABLE public.settings FROM rustodon_differential_writer",
    ),
    (
        "GRANT REFERENCES (var) ON TABLE public.settings TO rustodon_differential_writer",
        "REVOKE REFERENCES (var) ON TABLE public.settings FROM rustodon_differential_writer",
    ),
    (
        "GRANT SELECT ON TABLE public.quotes TO rustodon_differential_writer WITH GRANT OPTION",
        "REVOKE GRANT OPTION FOR SELECT ON TABLE public.quotes FROM rustodon_differential_writer",
    ),
    (
        "GRANT SELECT ON TABLE writer_privilege_probe.extra TO rustodon_differential_writer",
        "REVOKE SELECT ON TABLE writer_privilege_probe.extra FROM rustodon_differential_writer",
    ),
    (
        "GRANT USAGE ON SCHEMA writer_privilege_probe TO rustodon_differential_writer",
        "REVOKE USAGE ON SCHEMA writer_privilege_probe FROM rustodon_differential_writer",
    ),
    (
        "GRANT USAGE ON SCHEMA pgx_writer_privilege_probe TO rustodon_differential_writer",
        "REVOKE USAGE ON SCHEMA pgx_writer_privilege_probe FROM rustodon_differential_writer",
    ),
    (
        "GRANT USAGE ON SCHEMA pg_catalog TO rustodon_differential_writer",
        "REVOKE USAGE ON SCHEMA pg_catalog FROM rustodon_differential_writer",
    ),
    (
        "GRANT INSERT ON TABLE public.quotes TO PUBLIC",
        "REVOKE INSERT ON TABLE public.quotes FROM PUBLIC",
    ),
    (
        "GRANT SELECT ON TABLE public.account_statuses_cleanup_policies TO PUBLIC",
        "REVOKE SELECT ON TABLE public.account_statuses_cleanup_policies FROM PUBLIC",
    ),
    (
        "GRANT EXECUTE ON FUNCTION public.rustodon_refresh_instances() TO PUBLIC",
        "REVOKE EXECUTE ON FUNCTION public.rustodon_refresh_instances() FROM PUBLIC",
    ),
    (
        "GRANT SELECT ON TABLE pg_catalog.pg_authid TO PUBLIC",
        "REVOKE ALL ON TABLE pg_catalog.pg_authid FROM PUBLIC",
    ),
    (
        "GRANT SELECT ON TABLE information_schema.writer_privilege_probe TO PUBLIC",
        "REVOKE ALL ON TABLE information_schema.writer_privilege_probe FROM PUBLIC",
    ),
    (
        "GRANT SELECT (subname) ON TABLE pg_catalog.pg_subscription TO rustodon_differential_writer WITH GRANT OPTION",
        "REVOKE SELECT (subname) ON TABLE pg_catalog.pg_subscription FROM rustodon_differential_writer",
    ),
    (
        "GRANT EXECUTE ON FUNCTION pg_catalog.writer_privilege_probe() TO PUBLIC",
        "REVOKE ALL ON FUNCTION pg_catalog.writer_privilege_probe() FROM PUBLIC",
    ),
    (
        "GRANT USAGE ON TYPE writer_privilege_probe.extra_type TO rustodon_differential_writer",
        "REVOKE ALL ON TYPE writer_privilege_probe.extra_type FROM rustodon_differential_writer",
    ),
    (
        "GRANT USAGE ON LANGUAGE plpgsql TO rustodon_differential_writer",
        "REVOKE USAGE ON LANGUAGE plpgsql FROM rustodon_differential_writer",
    ),
    (
        "GRANT USAGE ON FOREIGN DATA WRAPPER writer_privilege_probe_fdw TO rustodon_differential_writer",
        "REVOKE USAGE ON FOREIGN DATA WRAPPER writer_privilege_probe_fdw FROM rustodon_differential_writer",
    ),
    (
        "GRANT USAGE ON FOREIGN DATA WRAPPER writer_privilege_probe_fdw TO PUBLIC",
        "REVOKE USAGE ON FOREIGN DATA WRAPPER writer_privilege_probe_fdw FROM PUBLIC",
    ),
    (
        "GRANT USAGE ON FOREIGN SERVER writer_privilege_probe_server TO rustodon_differential_writer",
        "REVOKE USAGE ON FOREIGN SERVER writer_privilege_probe_server FROM rustodon_differential_writer",
    ),
    (
        "GRANT USAGE ON FOREIGN SERVER writer_privilege_probe_server TO PUBLIC",
        "REVOKE USAGE ON FOREIGN SERVER writer_privilege_probe_server FROM PUBLIC",
    ),
    (
        "GRANT CREATE ON TABLESPACE pg_default TO rustodon_differential_writer",
        "REVOKE CREATE ON TABLESPACE pg_default FROM rustodon_differential_writer",
    ),
    (
        "GRANT CREATE ON TABLESPACE pg_default TO PUBLIC",
        "REVOKE CREATE ON TABLESPACE pg_default FROM PUBLIC",
    ),
    (
        "ALTER PUBLICATION writer_privilege_probe_publication OWNER TO rustodon_differential_writer",
        "ALTER PUBLICATION writer_privilege_probe_publication OWNER TO rustodon_fixture",
    ),
    (
        "GRANT SELECT ON LARGE OBJECT 2147483000 TO PUBLIC",
        "REVOKE ALL ON LARGE OBJECT 2147483000 FROM PUBLIC",
    ),
    (
        "GRANT UPDATE ON LARGE OBJECT 2147483000 TO rustodon_differential_writer",
        "REVOKE ALL ON LARGE OBJECT 2147483000 FROM rustodon_differential_writer",
    ),
    (
        "ALTER LARGE OBJECT 2147483000 OWNER TO rustodon_differential_writer",
        "ALTER LARGE OBJECT 2147483000 OWNER TO rustodon_fixture",
    ),
    (
        "GRANT CREATE ON SCHEMA pg_catalog TO PUBLIC",
        "REVOKE CREATE ON SCHEMA pg_catalog FROM PUBLIC",
    ),
    (
        "GRANT USAGE ON SEQUENCE public.quotes_id_seq TO rustodon_differential_writer",
        "REVOKE USAGE ON SEQUENCE public.quotes_id_seq FROM rustodon_differential_writer",
    ),
    (
        "GRANT EXECUTE ON FUNCTION public.writer_privilege_probe() TO rustodon_differential_writer",
        "REVOKE EXECUTE ON FUNCTION public.writer_privilege_probe() FROM rustodon_differential_writer",
    ),
    (
        "ALTER ROLE rustodon_differential_writer SET default_transaction_read_only = on",
        "ALTER ROLE rustodon_differential_writer RESET default_transaction_read_only",
    ),
    (
        "ALTER ROLE rustodon_differential_writer SET lo_compat_privileges = on",
        "ALTER ROLE rustodon_differential_writer RESET lo_compat_privileges",
    ),
    (
        "REVOKE SELECT ON TABLE public.tombstones FROM rustodon_differential_writer",
        "GRANT SELECT ON TABLE public.tombstones TO rustodon_differential_writer",
    ),
    (
        "ALTER FUNCTION public.rustodon_refresh_instances() SET search_path = public",
        "ALTER FUNCTION public.rustodon_refresh_instances() SET search_path = pg_catalog, public",
    ),
    (
        "ALTER FUNCTION public.rustodon_refresh_instances() OWNER TO rustodon_differential_writer",
        "ALTER FUNCTION public.rustodon_refresh_instances() OWNER TO rustodon_fixture; \
         GRANT EXECUTE ON FUNCTION public.rustodon_refresh_instances() TO rustodon_differential_writer",
    ),
    (
        "ALTER DATABASE rustodon_mastodon_v4_6_5_fixture OWNER TO rustodon_differential_writer",
        "ALTER DATABASE rustodon_mastodon_v4_6_5_fixture OWNER TO rustodon_fixture; \
         GRANT CONNECT ON DATABASE rustodon_mastodon_v4_6_5_fixture TO rustodon_differential_writer",
    ),
];

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
    let writer_url = std::env::var("RUSTODON_STARTUP_WRITE_DATABASE_URL")?;
    let port = unused_port()?;
    let child = ChildCleanup::new(
        command_with_writer("web", &runtime_url, port, Some(&writer_url)).spawn()?,
    );
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
    terminate(child.into_child())?;
    result
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn unsafe_writer_roles_bind_no_web_service() -> Result<(), Box<dyn std::error::Error>> {
    let runtime_url = std::env::var("RUSTODON_STARTUP_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_STARTUP_OWNER_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_STARTUP_WRITE_DATABASE_URL")?;
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query("CREATE ROLE rustodon_writer_group NOLOGIN")
        .execute(&mut owner)
        .await?;
    sqlx::query(
        "CREATE FUNCTION public.writer_privilege_probe() RETURNS void
         LANGUAGE plpgsql SECURITY DEFINER AS 'BEGIN NULL; END'",
    )
    .execute(&mut owner)
    .await?;
    sqlx::query("REVOKE ALL ON FUNCTION public.writer_privilege_probe() FROM PUBLIC")
        .execute(&mut owner)
        .await?;
    sqlx::query(
        "CREATE FUNCTION pg_catalog.writer_privilege_probe() RETURNS void
         LANGUAGE plpgsql SECURITY DEFINER AS 'BEGIN NULL; END'",
    )
    .execute(&mut owner)
    .await?;
    sqlx::query("REVOKE ALL ON FUNCTION pg_catalog.writer_privilege_probe() FROM PUBLIC")
        .execute(&mut owner)
        .await?;
    sqlx::raw_sql(
        "CREATE FOREIGN DATA WRAPPER writer_privilege_probe_fdw; \
         CREATE SERVER writer_privilege_probe_server \
           FOREIGN DATA WRAPPER writer_privilege_probe_fdw; \
         CREATE PUBLICATION writer_privilege_probe_publication \
           FOR TABLE public.accounts",
    )
    .execute(&mut owner)
    .await?;
    sqlx::query(
        "CREATE VIEW information_schema.writer_privilege_probe AS
         SELECT rolname FROM pg_catalog.pg_roles",
    )
    .execute(&mut owner)
    .await?;
    sqlx::query("REVOKE ALL ON information_schema.writer_privilege_probe FROM PUBLIC")
        .execute(&mut owner)
        .await?;
    sqlx::raw_sql(
        "CREATE SCHEMA writer_privilege_probe; \
         CREATE TABLE writer_privilege_probe.extra (id integer); \
         REVOKE ALL ON SCHEMA writer_privilege_probe FROM PUBLIC; \
         REVOKE ALL ON TABLE writer_privilege_probe.extra FROM PUBLIC; \
         CREATE SCHEMA pgx_writer_privilege_probe; \
         REVOKE ALL ON SCHEMA pgx_writer_privilege_probe FROM PUBLIC",
    )
    .execute(&mut owner)
    .await?;
    sqlx::query("CREATE TYPE writer_privilege_probe.extra_type AS ENUM ('one')")
        .execute(&mut owner)
        .await?;
    sqlx::query("SELECT pg_catalog.lo_create(2147483000::oid)")
        .execute(&mut owner)
        .await?;
    for &(enable, disable) in WRITER_PRIVILEGE_MUTATIONS {
        sqlx::query(enable).execute(&mut owner).await?;
        let port = unused_port()?;
        let output = output_with_timeout(command_with_writer(
            "web",
            &runtime_url,
            port,
            Some(&writer_url),
        ))?;
        assert_refused_code(&output, "PF_WRITE_DATABASE_PRIVILEGES", &writer_url);
        assert!(
            TcpStream::connect_timeout(
                &SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_millis(100)
            )
            .is_err()
        );
        let worker = output_with_timeout(command_with_writer(
            "worker",
            &runtime_url,
            unused_port()?,
            Some(&writer_url),
        ))?;
        assert_refused_code(&worker, "PF_WRITE_DATABASE_PRIVILEGES", &writer_url);
        sqlx::raw_sql(disable).execute(&mut owner).await?;
    }
    sqlx::query("DROP FUNCTION public.writer_privilege_probe()")
        .execute(&mut owner)
        .await?;
    sqlx::query("DROP FUNCTION pg_catalog.writer_privilege_probe()")
        .execute(&mut owner)
        .await?;
    sqlx::query("DROP VIEW information_schema.writer_privilege_probe")
        .execute(&mut owner)
        .await?;
    sqlx::query("SELECT pg_catalog.lo_unlink(2147483000::oid)")
        .execute(&mut owner)
        .await?;
    sqlx::query("DROP SCHEMA writer_privilege_probe CASCADE")
        .execute(&mut owner)
        .await?;
    sqlx::query("DROP PUBLICATION writer_privilege_probe_publication")
        .execute(&mut owner)
        .await?;
    sqlx::query("DROP SERVER writer_privilege_probe_server")
        .execute(&mut owner)
        .await?;
    sqlx::query("DROP FOREIGN DATA WRAPPER writer_privilege_probe_fdw")
        .execute(&mut owner)
        .await?;
    sqlx::query("DROP SCHEMA pgx_writer_privilege_probe CASCADE")
        .execute(&mut owner)
        .await?;
    sqlx::query("DROP ROLE rustodon_writer_group")
        .execute(&mut owner)
        .await?;
    Ok(())
}

// Keep these independent of the production policy: each missing permission must fail closed.
const EMOJI_INSERT_COLUMNS: &[&str] = &[
    "shortcode",
    "domain",
    "uri",
    "image_remote_url",
    "disabled",
    "visible_in_picker",
    "created_at",
    "updated_at",
];
const EMOJI_UPDATE_COLUMNS: &[&str] = &[
    "uri",
    "image_remote_url",
    "updated_at",
    "image_content_type",
    "image_file_name",
    "image_file_size",
    "image_storage_schema_version",
    "image_updated_at",
];

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn emoji_writer_privileges_are_required_and_exact() -> Result<(), Box<dyn std::error::Error>>
{
    let runtime_url = std::env::var("RUSTODON_STARTUP_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_STARTUP_OWNER_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_STARTUP_WRITE_DATABASE_URL")?;
    let environment = command_with_writer("web", &runtime_url, unused_port()?, Some(&writer_url))
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
        })
        .collect();
    let config = rustodon::config::Config::from_environment(&environment)?;
    assert!(
        rustodon::preflight::writer_diagnostics(&config)
            .await
            .is_empty()
    );
    let mut owner = PgConnection::connect(&owner_url).await?;
    let mut mutations = Vec::new();
    for (privilege, columns) in [
        ("INSERT", EMOJI_INSERT_COLUMNS),
        ("UPDATE", EMOJI_UPDATE_COLUMNS),
    ] {
        for column in columns {
            mutations.push((
                format!("REVOKE {privilege} ({column}) ON public.custom_emojis FROM rustodon_differential_writer"),
                format!("GRANT {privilege} ({column}) ON public.custom_emojis TO rustodon_differential_writer"),
            ));
        }
        mutations.push((
            format!("GRANT {privilege} ON public.custom_emojis TO rustodon_differential_writer"),
            format!("REVOKE {privilege} ON public.custom_emojis FROM rustodon_differential_writer; GRANT {privilege} ({}) ON public.custom_emojis TO rustodon_differential_writer", columns.join(", ")),
        ));
    }
    mutations.push((
        "REVOKE USAGE ON SEQUENCE public.custom_emojis_id_seq FROM rustodon_differential_writer"
            .into(),
        "GRANT USAGE ON SEQUENCE public.custom_emojis_id_seq TO rustodon_differential_writer"
            .into(),
    ));
    for privilege in [
        "UPDATE (disabled)",
        "UPDATE (visible_in_picker)",
        "UPDATE (category_id)",
        "UPDATE (shortcode)",
        "UPDATE (domain)",
        "INSERT (id)",
        "INSERT (category_id)",
    ] {
        mutations.push((
            format!("GRANT {privilege} ON public.custom_emojis TO rustodon_differential_writer"),
            format!("REVOKE {privilege} ON public.custom_emojis FROM rustodon_differential_writer"),
        ));
    }
    for privilege in ["SELECT", "UPDATE"] {
        mutations.push((
            format!("GRANT {privilege} ON SEQUENCE public.custom_emojis_id_seq TO rustodon_differential_writer"),
            format!("REVOKE {privilege} ON SEQUENCE public.custom_emojis_id_seq FROM rustodon_differential_writer"),
        ));
    }
    mutations.push((
        "GRANT UPDATE (image_remote_url) ON public.custom_emojis TO rustodon_differential_writer WITH GRANT OPTION".into(),
        "REVOKE GRANT OPTION FOR UPDATE (image_remote_url) ON public.custom_emojis FROM rustodon_differential_writer".into(),
    ));
    for (mutate, restore) in mutations {
        sqlx::raw_sql(&mutate).execute(&mut owner).await?;
        let diagnostics = rustodon::preflight::writer_diagnostics(&config).await;
        sqlx::raw_sql(&restore).execute(&mut owner).await?;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code() == "PF_WRITE_DATABASE_PRIVILEGES"),
            "accepted {mutate}: {diagnostics:?}"
        );
        assert!(
            rustodon::preflight::writer_diagnostics(&config)
                .await
                .is_empty(),
            "restore failed for {mutate}"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn least_privilege_writer_can_lock_account_deletion_requests()
-> Result<(), Box<dyn std::error::Error>> {
    let writer_url = std::env::var("RUSTODON_STARTUP_WRITE_DATABASE_URL")?;
    let mut writer = PgConnection::connect(&writer_url).await?;
    sqlx::query(
        "SELECT created_at FROM public.account_deletion_requests
         WHERE false FOR UPDATE",
    )
    .fetch_optional(&mut writer)
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn fatal_startup_binds_no_web_service_and_claims_no_worker_work()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime_url = std::env::var("RUSTODON_STARTUP_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_STARTUP_OWNER_DATABASE_URL")?;
    let port = unused_port()?;
    let mut web_command = command("web", &runtime_url, port);
    web_command.env("S3_ENABLED", "true");
    let web = output_with_timeout(web_command)?;
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
    let mut worker_command = command("worker", &runtime_url, unused_port()?);
    worker_command.env("S3_ENABLED", "true");
    let worker = output_with_timeout(worker_command)?;
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
    command_with_writer(mode, database_url, port, None)
}

fn command_with_writer(
    mode: &str,
    database_url: &str,
    port: u16,
    writer_url: Option<&str>,
) -> Command {
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
    if let Some(writer_url) = writer_url {
        command.env("WRITE_DATABASE_URL", writer_url);
    }
    command
}

struct ChildCleanup(Option<Child>);

impl ChildCleanup {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn into_child(mut self) -> Child {
        self.0.take().expect("child cleanup guard already consumed")
    }
}

impl Drop for ChildCleanup {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().ok().flatten().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = child.wait();
    }
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

fn terminate(mut child: Child) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()?;
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait()?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "server process did not stop within 30 seconds",
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let output = child.wait_with_output()?;
    #[cfg(unix)]
    let terminated_by_sigterm = output.status.signal() == Some(15);
    #[cfg(not(unix))]
    let terminated_by_sigterm = false;
    assert!(
        output.status.success() || terminated_by_sigterm,
        "child exited with {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn assert_refused(output: &Output, database_url: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PF_CONFIG_OBJECT_STORAGE_S3"));
    assert!(!stderr.contains("must-not-appear-secret"));
    assert!(!stderr.contains(database_url));
}

fn assert_refused_code(output: &Output, code: &str, secret: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(code), "missing {code} in {stderr}");
    assert!(!stderr.contains(secret));
}

fn output_with_timeout(mut command: Command) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait_with_output()?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "startup refusal command did not exit within 30 seconds",
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
