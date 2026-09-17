use sqlx::{Connection, PgConnection, Row};

#[test]
fn quote_authorization_delete_preserves_signed_activity_for_forwarding() {
    let worker = include_str!("../src/worker.rs");
    let writer = include_str!("../src/mastodon/write_repository.rs");

    assert!(
        worker.contains("DeleteQuoteAuthorization {\n            actor_uri,\n            authorization_uri,\n            activity,"),
        "typed authorization Deletes must retain the verified activity"
    );
    assert!(
        worker.matches(".then_some(&activity)").count() >= 2,
        "typed and scalar authorization Deletes must pass the signed activity to the writer"
    );
    assert!(
        writer.contains("record_remote_activity_forwarding_for_status_in("),
        "authorization revocation must reuse status-reach forwarding"
    );
}

#[test]
fn quote_delivery_rechecks_exact_identity_while_holding_the_send_lock() {
    let worker = include_str!("../src/worker.rs");
    let writer = include_str!("../src/mastodon/write_repository.rs");

    for field in [
        "quote_delivery_kind",
        "quote_request_uri",
        "quote_id",
        "quoting_status_id",
        "quoted_status_id",
    ] {
        assert!(
            worker.contains(field) && writer.contains(field),
            "quote delivery metadata must carry {field}"
        );
    }
    assert!(
        worker.contains("quote.id = $1 AND quote.status_id = $2 AND quote.quoted_status_id = $3")
    );
    assert!(worker.contains("quote.activity_uri = $4 AND quote.state = 0"));
    assert!(worker.contains(
        "let expected_state = if identity.kind == QuoteDeliveryKind::Accept {\n                1\n            } else {\n                2\n            };"
    ));
    assert!(worker.contains("($11 = 2 AND quote.state IN (2, 3))"));
    assert!(worker.contains(
        "WHERE (account_id = $1 AND target_account_id = $2) \\\n              OR (account_id = $2 AND target_account_id = $1)"
    ));
    assert!(
        worker
            .contains("hashtextextended(LEAST($1, $2)::text || ':' || GREATEST($1, $2)::text, 0)")
    );
    assert!(worker.contains("lease_generation = $3"));
    assert!(worker.contains("quote delivery metadata does not match its activity"));
    let guarded_import = writer
        .find("apply_remote_quote_request_instrument")
        .expect("QuoteRequest imports use a guarded writer path");
    let guarded_insert = writer[guarded_import..]
        .find("insert_remote_note(")
        .expect("guarded import eventually inserts the Note");
    let guarded_authorization = writer[guarded_import..]
        .find("writable_quote_target(")
        .expect("guarded import reauthorizes its target");
    assert!(guarded_authorization < guarded_insert);

    let fence = worker
        .find("let current = quote_delivery_is_current_and_locked(")
        .expect("quote delivery must take its final database fence");
    let send = worker[fence..]
        .find(".post_signed_json")
        .expect("the signed HTTP send follows the fence");
    let commit = worker[fence..]
        .find("transaction\n                .commit()")
        .expect("the fence transaction ends after the HTTP attempt");
    assert!(send < commit, "the quote row lock must cover the HTTP send");

    assert!(writer.contains("arguments #>> '{body,object,id}' = $4"));
    assert!(writer.contains("AND (lease_owner IS NULL OR lease_expires_at <= clock_timestamp())"));
}

#[test]
fn quote_delivery_writer_can_renew_its_durable_lease() {
    let grants = include_str!("../docs/mastodon-writer-grants.sql");
    let preflight = include_str!("../src/preflight.rs");

    assert!(
        grants
            .contains("GRANT UPDATE (lease_expires_at, updated_at) ON TABLE rustodon.durable_jobs")
    );
    assert!(preflight.contains(
        "has_column_privilege(role.oid, 'rustodon.durable_jobs', 'lease_expires_at', 'UPDATE')"
    ));
    assert!(preflight.contains(
        "has_column_privilege(role.oid, 'rustodon.durable_jobs', 'updated_at', 'UPDATE')"
    ));
    assert!(
        preflight
            .contains("'durable_jobs', 'idempotency_keys', 'ordering_markers', 'outbox_events'")
    );
}

#[tokio::test]
#[ignore = "requires the disposable restored Mastodon schema and writer role"]
async fn quote_writer_privileges_match_schema_lifecycle() -> Result<(), Box<dyn std::error::Error>>
{
    let writer_url = std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")?;
    let mut writer = PgConnection::connect(&writer_url).await?;

    let default: String = sqlx::query_scalar(
        "SELECT pg_catalog.pg_get_expr(attribute.adbin, attribute.adrelid) \
           FROM pg_catalog.pg_attrdef attribute \
           JOIN pg_catalog.pg_class relation ON relation.oid = attribute.adrelid \
           JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace \
           JOIN pg_catalog.pg_attribute column_definition \
             ON column_definition.attrelid = relation.oid \
            AND column_definition.attnum = attribute.adnum \
          WHERE namespace.nspname = 'public' AND relation.relname = 'quotes' \
            AND column_definition.attname = 'id'",
    )
    .fetch_one(&mut writer)
    .await?;
    assert_eq!(default, "timestamp_id('quotes'::text)");

    let row = sqlx::query(
        "SELECT \
           has_table_privilege(current_user, 'public.quotes', 'SELECT') AS can_select, \
           has_table_privilege(current_user, 'public.quotes', 'INSERT') AS can_insert, \
           has_table_privilege(current_user, 'public.quotes', 'UPDATE') AS can_update, \
           has_table_privilege(current_user, 'public.quotes', 'DELETE') AS can_delete, \
           has_table_privilege(current_user, 'public.quotes', 'TRUNCATE') AS can_truncate, \
           has_table_privilege(current_user, 'public.quotes', 'REFERENCES') AS can_reference, \
           has_table_privilege(current_user, 'public.quotes', 'TRIGGER') AS can_trigger, \
           has_sequence_privilege(current_user, 'public.quotes_id_seq', 'USAGE') AS seq_usage, \
           has_sequence_privilege(current_user, 'public.quotes_id_seq', 'SELECT') AS seq_select, \
           has_sequence_privilege(current_user, 'public.quotes_id_seq', 'UPDATE') AS seq_update",
    )
    .fetch_one(&mut writer)
    .await?;
    assert!(row.get::<bool, _>("can_select"));
    assert!(row.get::<bool, _>("can_insert"));
    assert!(row.get::<bool, _>("can_update"));
    assert!(!row.get::<bool, _>("can_delete"));
    assert!(!row.get::<bool, _>("can_truncate"));
    assert!(!row.get::<bool, _>("can_reference"));
    assert!(!row.get::<bool, _>("can_trigger"));
    assert!(row.get::<bool, _>("seq_usage"));
    assert!(!row.get::<bool, _>("seq_select"));
    assert!(!row.get::<bool, _>("seq_update"));

    let durable = sqlx::query(
        "SELECT \
           has_table_privilege(current_user, 'rustodon.durable_jobs', 'UPDATE') AS table_update, \
           has_column_privilege(current_user, 'rustodon.durable_jobs', 'lease_expires_at', 'UPDATE') AS lease_update, \
           has_column_privilege(current_user, 'rustodon.durable_jobs', 'updated_at', 'UPDATE') AS timestamp_update, \
           has_column_privilege(current_user, 'rustodon.durable_jobs', 'arguments', 'UPDATE') AS arguments_update",
    )
    .fetch_one(&mut writer)
    .await?;
    assert!(!durable.get::<bool, _>("table_update"));
    assert!(durable.get::<bool, _>("lease_update"));
    assert!(durable.get::<bool, _>("timestamp_update"));
    assert!(!durable.get::<bool, _>("arguments_update"));
    Ok(())
}

#[test]
fn quote_authorization_delete_is_durable_before_quote_attachment() {
    let writer = include_str!("../src/mastodon/write_repository.rs");
    let delete = writer
        .find("pub(crate) async fn apply_remote_quote_authorization_delete")
        .expect("authorization Delete handler exists");
    let marker = writer[delete..]
        .find(
            "insert_remote_note_tombstone(&mut transaction, source_account_id, authorization_uri)",
        )
        .expect("authorization Delete must persist a durable marker");
    let quote_lookup = writer[delete..]
        .find("WHERE quote.approval_uri = $1 AND quote.quoted_account_id = $2")
        .expect("authorization Delete quote lookup exists");
    assert!(
        marker < quote_lookup,
        "Delete-before-attachment must persist first"
    );

    let apply = writer
        .find("pub(crate) async fn apply_remote_quote_authorization(")
        .expect("authorization import handler exists");
    let decision = writer
        .find("pub(crate) async fn apply_remote_quote_decision")
        .expect("quote decision handler exists");
    let decision_body = &writer[decision..apply];
    let request_lock = decision_body
        .find("lock_remote_interaction(&mut transaction, request_uri)")
        .expect("decisions lock the QuoteRequest URI");
    let authorization_lock = decision_body
        .find("lock_remote_interaction(\n                &mut transaction,\n                result_uri.expect")
        .expect("accepted decisions lock the authorization URI");
    let quote_lock = decision_body
        .find("FOR UPDATE OF quote, instrument")
        .expect("decisions lock their quote");
    assert!(request_lock < authorization_lock && authorization_lock < quote_lock);
    assert!(decision_body.contains("0 => 2"));
    assert!(decision_body.contains("1 => 3"));

    let apply_body = &writer[apply..delete];
    assert!(apply_body.contains(
        "remote_interaction_tombstoned(&mut transaction, quoted_account_id, approval_uri)"
    ));
    assert!(
        apply_body.contains("if state != 0"),
        "rejected and revoked quotes must not be resurrected by stale authorization"
    );
    assert!(apply_body.contains("WHERE id = $1 AND state = 0"));
}

#[test]
fn quote_lifecycle_does_not_require_quote_delete_privilege() {
    let writer = include_str!("../src/mastodon/write_repository.rs");
    assert!(
        !writer.contains("DELETE FROM quotes"),
        "quote lifecycle must use state transitions so the writer keeps least privilege"
    );
}
