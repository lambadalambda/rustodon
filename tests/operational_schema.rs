use std::collections::BTreeSet;

use rustodon::operational_schema::{
    CURRENT_VERSION, MigrationError, MigrationRecord, migrate, migration_plan,
};
use sqlx::{Connection, PgConnection};
use tokio::time::{Duration, timeout};

const TABLES: &[&str] = &[
    "domain_health",
    "durable_jobs",
    "heartbeats",
    "idempotency_keys",
    "ordering_markers",
    "outbox_events",
];

#[test]
fn migration_plan_requires_an_exact_known_prefix() {
    assert_eq!(migration_plan(&[]).unwrap(), vec![1]);
    let current = vec![MigrationRecord::known(1).expect("migration 1 exists")];
    assert!(migration_plan(&current).unwrap().is_empty());

    let unknown = vec![MigrationRecord {
        version: CURRENT_VERSION + 1,
        checksum: [0; 32],
    }];
    assert!(matches!(
        migration_plan(&unknown),
        Err(MigrationError::UnknownVersion(2))
    ));

    let wrong_checksum = vec![MigrationRecord {
        version: 1,
        checksum: [0; 32],
    }];
    assert!(matches!(
        migration_plan(&wrong_checksum),
        Err(MigrationError::ChecksumMismatch(1))
    ));
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn operational_schema_lifecycle_is_isolated_and_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let mut connection = PgConnection::connect(&url).await?;

    migrate(&mut connection).await?;
    assert_schema(&mut connection).await?;
    assert_active_record_migration_lock(&url).await?;

    sqlx::raw_sql("DROP SCHEMA rustodon CASCADE; CREATE SCHEMA rustodon")
        .execute(&mut connection)
        .await?;
    let attempts = (0..8)
        .map(|_| {
            let url = url.clone();
            async move {
                let mut connection = PgConnection::connect(&url).await?;
                migrate(&mut connection).await
            }
        })
        .collect::<Vec<_>>();
    for attempt in futures_util::future::join_all(attempts).await {
        attempt?;
    }
    assert_schema(&mut connection).await?;
    let first_applied_at = applied_at(&mut connection).await?;
    migrate(&mut connection).await?;
    assert_eq!(applied_at(&mut connection).await?, first_applied_at);

    sqlx::query("DROP SCHEMA rustodon CASCADE")
        .execute(&mut connection)
        .await?;
    let attempts = (0..8)
        .map(|_| {
            let url = url.clone();
            async move {
                let mut connection = PgConnection::connect(&url).await?;
                migrate(&mut connection).await
            }
        })
        .collect::<Vec<_>>();
    for attempt in futures_util::future::join_all(attempts).await {
        attempt?;
    }
    assert_schema(&mut connection).await?;

    sqlx::query("UPDATE rustodon.schema_migrations SET version = $1")
        .bind(CURRENT_VERSION + 1)
        .execute(&mut connection)
        .await?;
    let error = migrate(&mut connection)
        .await
        .expect_err("future schema versions must be rejected");
    assert!(matches!(error, MigrationError::UnknownVersion(2)));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT version FROM rustodon.schema_migrations")
            .fetch_one(&mut connection)
            .await?,
        2
    );

    sqlx::query("UPDATE rustodon.schema_migrations SET version = 1, checksum = $1")
        .bind([0_u8; 32].as_slice())
        .execute(&mut connection)
        .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::ChecksumMismatch(1))
    ));

    let checksum = MigrationRecord::known(1).unwrap().checksum;
    sqlx::query("UPDATE rustodon.schema_migrations SET checksum = $1")
        .bind(checksum.as_slice())
        .execute(&mut connection)
        .await?;
    sqlx::query("ALTER TABLE rustodon.heartbeats ADD COLUMN drift text")
        .execute(&mut connection)
        .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::SchemaDrift(_))
    ));
    sqlx::query("ALTER TABLE rustodon.heartbeats DROP COLUMN drift")
        .execute(&mut connection)
        .await?;
    migrate(&mut connection).await?;

    sqlx::raw_sql(
        "DROP SCHEMA rustodon CASCADE; CREATE SCHEMA rustodon; \
         CREATE TABLE rustodon.unknown (id bigint)",
    )
    .execute(&mut connection)
    .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::UnversionedSchema)
    ));
    sqlx::raw_sql(
        "DROP SCHEMA rustodon CASCADE; CREATE SCHEMA rustodon; \
         CREATE TYPE rustodon.unknown AS ENUM ('x')",
    )
    .execute(&mut connection)
    .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::UnversionedSchema)
    ));
    sqlx::query("DROP SCHEMA rustodon CASCADE")
        .execute(&mut connection)
        .await?;
    migrate(&mut connection).await?;
    assert_schema(&mut connection).await?;
    Ok(())
}

async fn assert_active_record_migration_lock(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut blocker = PgConnection::connect(url).await?;
    let database = sqlx::query_scalar::<_, String>("SELECT pg_catalog.current_database()")
        .fetch_one(&mut blocker)
        .await?;
    let lock_id = 2_053_462_845_i64 * i64::from(crc32(database.as_bytes()));
    sqlx::query("SELECT pg_catalog.pg_advisory_lock($1)")
        .bind(lock_id)
        .execute(&mut blocker)
        .await?;
    let mut migrator = PgConnection::connect(url).await?;
    assert!(
        timeout(Duration::from_millis(250), migrate(&mut migrator))
            .await
            .is_err(),
        "migration did not wait for Active Record's advisory lock"
    );
    sqlx::query("SELECT pg_catalog.pg_advisory_unlock($1)")
        .bind(lock_id)
        .execute(&mut blocker)
        .await?;
    timeout(Duration::from_secs(30), migrate(&mut migrator)).await??;
    Ok(())
}

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |mut crc, byte| {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
        crc
    })
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn operational_schema_rejects_namespace_and_access_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let admin_url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let mut connection = PgConnection::connect(&url).await?;
    let mut admin = PgConnection::connect(&admin_url).await?;
    migrate(&mut connection).await?;

    sqlx::query("GRANT SELECT ON rustodon.heartbeats TO rustodon_fixture")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("REVOKE SELECT ON rustodon.heartbeats FROM rustodon_fixture")
        .execute(&mut connection)
        .await?;

    sqlx::query("CREATE RULE heartbeat_noop AS ON UPDATE TO rustodon.heartbeats DO ALSO NOTHING")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP RULE heartbeat_noop ON rustodon.heartbeats")
        .execute(&mut connection)
        .await?;

    sqlx::query(
        "CREATE TRIGGER heartbeat_noop BEFORE UPDATE ON rustodon.heartbeats \
         FOR EACH ROW EXECUTE FUNCTION pg_catalog.suppress_redundant_updates_trigger()",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP TRIGGER heartbeat_noop ON rustodon.heartbeats")
        .execute(&mut connection)
        .await?;

    sqlx::query("CREATE POLICY heartbeat_policy ON rustodon.heartbeats USING (true)")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP POLICY heartbeat_policy ON rustodon.heartbeats")
        .execute(&mut connection)
        .await?;

    sqlx::query(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA rustodon \
         GRANT SELECT ON TABLES TO rustodon_fixture",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA rustodon \
         REVOKE SELECT ON TABLES FROM rustodon_fixture",
    )
    .execute(&mut connection)
    .await?;

    sqlx::raw_sql(
        "CREATE COLLATION rustodon.\"C\" FROM pg_catalog.\"C\"; \
         ALTER TABLE rustodon.domain_health ALTER COLUMN last_error \
           TYPE text COLLATE rustodon.\"C\"",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::raw_sql(
        "ALTER TABLE rustodon.domain_health ALTER COLUMN last_error \
           TYPE text COLLATE pg_catalog.\"default\"; \
         DROP COLLATION rustodon.\"C\"",
    )
    .execute(&mut connection)
    .await?;

    sqlx::query(
        "CREATE TEXT SEARCH DICTIONARY rustodon.unexpected \
         (TEMPLATE = pg_catalog.simple)",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP TEXT SEARCH DICTIONARY rustodon.unexpected")
        .execute(&mut connection)
        .await?;

    sqlx::query("CREATE TYPE rustodon.unexpected AS ENUM ('x')")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP TYPE rustodon.unexpected")
        .execute(&mut connection)
        .await?;

    sqlx::query("COMMENT ON TABLE rustodon.heartbeats IS 'unexpected'")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("COMMENT ON TABLE rustodon.heartbeats IS NULL")
        .execute(&mut connection)
        .await?;

    sqlx::query(
        "COMMENT ON CONSTRAINT durable_jobs_kind_check ON rustodon.durable_jobs \
         IS 'unexpected'",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("COMMENT ON CONSTRAINT durable_jobs_kind_check ON rustodon.durable_jobs IS NULL")
        .execute(&mut connection)
        .await?;

    sqlx::query(
        "REVOKE SELECT, UPDATE ON SEQUENCE rustodon.durable_jobs_id_seq \
         FROM rustodon_schema_migrator",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query(
        "GRANT SELECT, UPDATE, USAGE ON SEQUENCE rustodon.durable_jobs_id_seq \
         TO rustodon_schema_migrator",
    )
    .execute(&mut connection)
    .await?;

    sqlx::query("ALTER EXTENSION plpgsql ADD SCHEMA rustodon")
        .execute(&mut admin)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("ALTER EXTENSION plpgsql DROP SCHEMA rustodon")
        .execute(&mut admin)
        .await?;

    sqlx::query("ALTER EXTENSION plpgsql ADD TABLE rustodon.heartbeats")
        .execute(&mut admin)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("ALTER EXTENSION plpgsql DROP TABLE rustodon.heartbeats")
        .execute(&mut admin)
        .await?;

    sqlx::query("CREATE VIEW public.rustodon_dependency AS SELECT * FROM rustodon.heartbeats")
        .execute(&mut admin)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP VIEW public.rustodon_dependency")
        .execute(&mut admin)
        .await?;

    sqlx::raw_sql(
        "CREATE FUNCTION public.rustodon_test_event_trigger() RETURNS event_trigger \
           LANGUAGE plpgsql AS $$ BEGIN END $$; \
         CREATE EVENT TRIGGER rustodon_test_event_trigger ON ddl_command_end \
           EXECUTE FUNCTION public.rustodon_test_event_trigger()",
    )
    .execute(&mut admin)
    .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::UnsupportedMastodonSchema(_))
    ));
    sqlx::raw_sql(
        "DROP EVENT TRIGGER rustodon_test_event_trigger; \
         DROP FUNCTION public.rustodon_test_event_trigger()",
    )
    .execute(&mut admin)
    .await?;

    sqlx::query("CREATE PUBLICATION rustodon_test_publication FOR ALL TABLES")
        .execute(&mut admin)
        .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::UnsupportedMastodonSchema(_))
    ));
    sqlx::query("DROP PUBLICATION rustodon_test_publication")
        .execute(&mut admin)
        .await?;

    sqlx::raw_sql(
        "REASSIGN OWNED BY rustodon_schema_migrator TO rustodon_displaced_owner; \
         GRANT USAGE ON SCHEMA public TO rustodon_displaced_owner; \
         GRANT SELECT ON ALL TABLES IN SCHEMA public TO rustodon_displaced_owner; \
         GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO rustodon_displaced_owner; \
         SET ROLE rustodon_displaced_owner",
    )
    .execute(&mut admin)
    .await?;
    let ownership_result = migrate(&mut admin).await;
    sqlx::raw_sql(
        "RESET ROLE; \
         REASSIGN OWNED BY rustodon_displaced_owner TO rustodon_schema_migrator; \
         REVOKE USAGE ON SCHEMA public FROM rustodon_displaced_owner; \
         REVOKE SELECT ON ALL TABLES IN SCHEMA public FROM rustodon_displaced_owner; \
         REVOKE SELECT ON ALL SEQUENCES IN SCHEMA public FROM rustodon_displaced_owner",
    )
    .execute(&mut admin)
    .await?;
    assert!(matches!(
        ownership_result,
        Err(MigrationError::SchemaDrift(_))
    ));

    migrate(&mut connection).await?;
    Ok(())
}

async fn assert_schema_drift(connection: &mut PgConnection) {
    let result = migrate(connection).await;
    assert!(
        matches!(result, Err(MigrationError::SchemaDrift(_))),
        "expected schema drift, got {result:?}"
    );
}

async fn assert_schema(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    let tables = sqlx::query_scalar::<_, String>(
        "SELECT c.relname::text FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'rustodon' AND c.relkind = 'r' \
           AND c.relname <> 'schema_migrations' ORDER BY c.relname",
    )
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(
        tables,
        TABLES
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT version FROM rustodon.schema_migrations")
            .fetch_one(&mut *connection)
            .await?,
        CURRENT_VERSION
    );
    Ok(())
}

async fn applied_at(connection: &mut PgConnection) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT applied_at::text FROM rustodon.schema_migrations WHERE version = 1")
        .fetch_one(connection)
        .await
}
