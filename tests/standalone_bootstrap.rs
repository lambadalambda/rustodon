#![cfg(feature = "test-support")]

use std::path::PathBuf;

use rustodon::bootstrap::{BootstrapOutcome, BootstrapRequest, bootstrap_instance};
use rustodon::{operational_schema, preflight};
use sqlx::{Connection, PgConnection};

fn required_environment(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for this ignored test"))
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL 14 database and empty media root"]
#[allow(clippy::too_many_lines, clippy::manual_assert_eq)]
async fn standalone_bootstrap_installs_and_verifies_exact_baseline() {
    let database_url = required_environment("RUSTODON_BOOTSTRAP_DATABASE_URL");
    let runtime_database_url = required_environment("RUSTODON_BOOTSTRAP_RUNTIME_DATABASE_URL");
    let writer_database_url = required_environment("RUSTODON_BOOTSTRAP_WRITER_DATABASE_URL");
    let runtime_role = required_environment("RUSTODON_BOOTSTRAP_RUNTIME_ROLE");
    let writer_role = required_environment("RUSTODON_BOOTSTRAP_WRITER_ROLE");
    let media_root = PathBuf::from(required_environment("RUSTODON_BOOTSTRAP_MEDIA_ROOT"));
    let mut connection = PgConnection::connect(&database_url)
        .await
        .expect("connect installer database");
    let request = BootstrapRequest {
        admin_username: "alice",
        admin_email: "alice@bootstrap.invalid",
        admin_password: "bootstrap-test-password",
        site_title: "Bootstrap Test",
        runtime_role: &runtime_role,
        writer_role: &writer_role,
        media_root: &media_root,
    };

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
    .execute(&mut connection)
    .await
    .expect("set conflicting database role setting");
    assert!(bootstrap_instance(&mut connection, &request).await.is_err());
    sqlx::query(&format!(
        "ALTER ROLE {quoted_runtime} IN DATABASE {quoted_database} RESET default_transaction_read_only"
    ))
    .execute(&mut connection)
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
}
