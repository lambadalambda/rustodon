use rustodon::jobs::{JobSpec, Lane};
use rustodon::mastodon::{AccountProfileValue, MediaAttachmentCreate, local_uploads::*};
use serde_json::json;
use sqlx::{Connection, PgConnection};

async fn database() -> Result<PgConnection, Box<dyn std::error::Error>> {
    Ok(PgConnection::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?).await?)
}

/// Requires the pinned restored public schema, but no codecs or media runtime.
#[tokio::test]
#[ignore = "requires a disposable restored PostgreSQL database"]
async fn durable_upload_schema_contract() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = database().await?;
    rustodon::operational_schema::migrate(&mut db).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT to_regclass('rustodon.local_uploads') IS NOT NULL")
            .fetch_one(&mut db)
            .await?;
    assert!(
        exists,
        "filename-null rollback staging has no durable upload owner"
    );
    // Simulate precisely the previous applied prefix, then exercise additive upgrade.
    sqlx::raw_sql("DROP TABLE rustodon.activity_members, rustodon.activity_buckets, rustodon.local_uploads; DELETE FROM rustodon.schema_migrations WHERE version IN (5, 6)")
        .execute(&mut db).await?;
    sqlx::query("CREATE ROLE upload_upgrade_writer")
        .execute(&mut db)
        .await?;
    rustodon::operational_schema::migrate_with_writer_role(&mut db, Some("upload_upgrade_writer"))
        .await?;
    for privilege in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
        let granted: bool = sqlx::query_scalar(
            "SELECT has_table_privilege('upload_upgrade_writer', 'rustodon.local_uploads', $1)",
        )
        .bind(privilege)
        .fetch_one(&mut db)
        .await?;
        assert!(granted);
    }
    sqlx::raw_sql("REVOKE ALL ON rustodon.local_uploads, rustodon.activity_buckets, rustodon.activity_members FROM upload_upgrade_writer; DROP ROLE upload_upgrade_writer")
        .execute(&mut db).await?;
    rustodon::operational_schema::migrate(&mut db).await?;
    Ok(())
}

fn job(id: UploadIdentity) -> JobSpec {
    // A test-only intent: this fixture never runs a dispatcher or a worker.
    JobSpec::new(
        Lane::Maintenance,
        "test.upload_ownership",
        json!({
            "account_id": id.account_id, "media_id": id.media_id, "generation": id.generation,
        }),
    )
    .logical_key(format!(
        "mastodon:media:{}:upload:{}",
        id.media_id, id.generation
    ))
}

fn output() -> MediaAttachmentCreate {
    MediaAttachmentCreate {
        media_type: 0,
        content_type: "image/png".into(),
        file_name: "output.png".into(),
        file_size: 123,
        file_meta: json!({"original": {"width": 12, "height": 13}, "focus": {"x": -1, "y": -1}}),
        blurhash: None,
        description: Some("stale processor description".into()),
        focus: AccountProfileValue::Unchanged,
    }
}

#[tokio::test]
#[ignore = "requires a disposable restored PostgreSQL database"]
#[allow(clippy::too_many_lines)]
async fn durable_upload_lifecycle_contract() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = database().await?;
    rustodon::operational_schema::migrate(&mut db).await?;
    let account: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE domain IS NULL AND id > 0 ORDER BY id LIMIT 1",
    )
    .fetch_one(&mut db)
    .await?;
    let input = RawInput {
        mime: "image/png",
        size: 123,
        sha256: &[7; 32],
    };
    let mut tx = db.begin().await?;
    let interrupted = stage_in(&mut tx, account, 70, &input).await?;
    discard_staging_in(&mut tx, interrupted).await?;
    assert!(load_in(&mut tx, interrupted).await?.is_some());
    assert!(
        accept_in(&mut tx, interrupted, &job(interrupted))
            .await
            .is_err()
    );
    forget_orphan_in(&mut tx, interrupted).await?;
    tx.rollback().await?;
    // Stage commit is the ownership boundary before raw write. No legacy rollback job.
    let mut tx = db.begin().await?;
    let id = stage_in(&mut tx, account, 71, &input).await?;
    assert!(load_in(&mut tx, id).await?.is_some());
    let legacy: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rustodon.outbox_events WHERE logical_key = $1")
            .bind(format!("mastodon:media:{}:rollback_create", id.media_id))
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(legacy, 0);
    tx.commit().await?;
    let mut tx = db.begin().await?;
    let staged = load_in(&mut tx, id).await?.unwrap();
    assert!(!staged.accepted);
    assert_eq!(staged.raw_path, raw_path(id));
    assert_eq!(staged.raw_sha256, vec![7; 32]);
    assert!(claim_in(&mut tx, id).await.is_err());
    assert!(forget_orphan_in(&mut tx, id).await.is_err());
    accept_in(&mut tx, id, &job(id)).await?;
    tx.rollback().await?;
    let mut tx = db.begin().await?;
    assert!(!load_in(&mut tx, id).await?.unwrap().accepted);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rustodon.outbox_events WHERE logical_key = $1")
            .bind(job(id).logical_key_value())
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(count, 0, "acceptance and enqueue roll back together");
    accept_in(&mut tx, id, &job(id)).await?;
    accept_in(&mut tx, id, &job(id)).await?;
    assert!(discard_staging_in(&mut tx, id).await.is_err());
    let stale_id = UploadIdentity {
        generation: 72,
        ..id
    };
    assert!(claim_in(&mut tx, stale_id).await.is_err());
    let stale = claim_in(&mut tx, id).await?;
    let claim = claim_in(&mut tx, id).await?;
    assert!(
        register_outputs_in(&mut tx, stale, &output())
            .await
            .is_err()
    );
    assert!(publish_in(&mut tx, claim, &output()).await.is_err());
    let mut invalid = output();
    invalid.file_name = "../foreign.png".into();
    assert!(register_outputs_in(&mut tx, claim, &invalid).await.is_err());
    register_outputs_in(&mut tx, claim, &output()).await?;
    let paths = load_in(&mut tx, id).await?.unwrap().output_paths;
    assert_eq!(paths.len(), 2);
    let mut changed = output();
    changed.file_name = "changed.png".into();
    assert!(register_outputs_in(&mut tx, claim, &changed).await.is_err());
    tx.commit().await?;
    // Simulate the future writer's durable file installation. No filesystem or codec
    // claim is made here: this is the transaction publication boundary only.
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE media_attachments SET description = 'latest description', file_meta = $2::json WHERE id = $1")
        .bind(id.media_id).bind(json!({"focus": {"x": 0.5, "y": 0.25}})).execute(&mut *tx).await?;
    assert!(publish_in(&mut tx, stale, &output()).await.is_err());
    publish_in(&mut tx, claim, &output()).await?;
    publish_in(&mut tx, claim, &output()).await?;
    let ready: (Option<String>, serde_json::Value, i32) = sqlx::query_as(
        "SELECT description, file_meta, processing FROM media_attachments WHERE id = $1",
    )
    .bind(id.media_id)
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(ready.0.as_deref(), Some("latest description"));
    assert_eq!(ready.1["focus"], json!({"x": 0.5, "y": 0.25}));
    assert_eq!(ready.2, 2);
    assert!(claim_in(&mut tx, id).await.is_err());
    assert!(abandon_in(&mut tx, claim).await.is_err());
    // Deletion cannot cascade away the raw or output cleanup responsibility.
    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(id.media_id)
        .execute(&mut *tx)
        .await?;
    assert_eq!(load_in(&mut tx, id).await?.unwrap().output_paths, paths);
    assert!(publish_in(&mut tx, claim, &output()).await.is_err());
    forget_orphan_in(&mut tx, id).await?;
    assert!(load_in(&mut tx, id).await?.is_none());
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key = $1")
        .bind(job(id).logical_key_value())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable restored PostgreSQL database and role administration"]
async fn durable_upload_restricted_roles() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = database().await?;
    rustodon::operational_schema::migrate(&mut db).await?;
    let mut tx = db.begin().await?;
    sqlx::raw_sql(
        "CREATE ROLE upload_contract_reader; CREATE ROLE upload_contract_writer;
        GRANT USAGE ON SCHEMA rustodon, public TO upload_contract_reader, upload_contract_writer;
        GRANT SELECT, INSERT, UPDATE, DELETE ON rustodon.local_uploads TO upload_contract_writer;
        GRANT SELECT ON accounts TO upload_contract_writer;
        GRANT SELECT, INSERT, UPDATE, DELETE ON media_attachments TO upload_contract_writer;
        GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO upload_contract_writer",
    )
    .execute(&mut *tx)
    .await?;
    for privilege in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
        let reader: bool = sqlx::query_scalar(
            "SELECT has_table_privilege('upload_contract_reader', 'rustodon.local_uploads', $1)",
        )
        .bind(privilege)
        .fetch_one(&mut *tx)
        .await?;
        assert!(!reader);
        let writer: bool = sqlx::query_scalar(
            "SELECT has_table_privilege('upload_contract_writer', 'rustodon.local_uploads', $1)",
        )
        .bind(privilege)
        .fetch_one(&mut *tx)
        .await?;
        assert!(writer);
    }
    sqlx::query("SET LOCAL ROLE upload_contract_writer")
        .execute(&mut *tx)
        .await?;
    let account: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE domain IS NULL AND id > 0 ORDER BY id LIMIT 1",
    )
    .fetch_one(&mut *tx)
    .await?;
    let id = stage_in(
        &mut tx,
        account,
        19,
        &RawInput {
            mime: "audio/mpeg",
            size: 100,
            sha256: &[1; 32],
        },
    )
    .await?;
    assert_eq!(load_in(&mut tx, id).await?.unwrap().raw_mime, "audio/mpeg");
    tx.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable restored PostgreSQL database"]
async fn ready_upload_retirement_preserves_public_owner() -> Result<(), Box<dyn std::error::Error>>
{
    let mut db = database().await?;
    rustodon::operational_schema::migrate(&mut db).await?;
    let mut tx = db.begin().await?;
    let account = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE domain IS NULL AND id > 0 ORDER BY id LIMIT 1",
    )
    .fetch_one(&mut *tx)
    .await?;
    let id = stage_in(
        &mut tx,
        account,
        81,
        &RawInput {
            mime: "image/png",
            size: 123,
            sha256: &[8; 32],
        },
    )
    .await?;
    assert!(retire_ready_in(&mut tx, id).await.is_err());
    accept_in(&mut tx, id, &job(id)).await?;
    let claim = claim_in(&mut tx, id).await?;
    register_outputs_in(&mut tx, claim, &output()).await?;
    assert!(retire_ready_in(&mut tx, id).await.is_err());
    publish_in(&mut tx, claim, &output()).await?;
    assert!(
        retire_ready_in(
            &mut tx,
            UploadIdentity {
                generation: 82,
                ..id
            }
        )
        .await
        .is_err()
    );
    retire_ready_in(&mut tx, id).await?;
    assert!(load_in(&mut tx, id).await?.is_none());
    let ready: bool = sqlx::query_scalar("SELECT processing = 2 AND file_file_name = 'output.png' FROM media_attachments WHERE id = $1")
        .bind(id.media_id).fetch_one(&mut *tx).await?;
    assert!(ready);
    tx.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable restored PostgreSQL database"]
async fn terminal_upload_retains_failed_row_without_filenames()
-> Result<(), Box<dyn std::error::Error>> {
    let mut db = database().await?;
    rustodon::operational_schema::migrate(&mut db).await?;
    let account: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE domain IS NULL AND id > 0 ORDER BY id LIMIT 1",
    )
    .fetch_one(&mut db)
    .await?;
    let mut tx = db.begin().await?;
    let id = stage_in(
        &mut tx,
        account,
        1,
        &RawInput {
            mime: "video/webm",
            size: 123,
            sha256: &[0; 32],
        },
    )
    .await?;
    accept_in(&mut tx, id, &job(id)).await?;
    let claim = claim_in(&mut tx, id).await?;
    abandon_in(&mut tx, claim).await?;
    let row: Option<(Option<i32>, Option<String>)> =
        sqlx::query_as("SELECT processing, file_file_name FROM media_attachments WHERE id = $1")
            .bind(id.media_id)
            .fetch_optional(&mut *tx)
            .await?;
    assert_eq!(
        row,
        Some((Some(3), None)),
        "accepted failure must remain pollable, not 404"
    );
    assert!(claim_in(&mut tx, id).await.is_err());
    assert!(
        load_in(&mut tx, id).await?.is_some(),
        "cleanup ownership remains until durable unlink"
    );
    // Simulated durable unlink: cleanup may retire ownership but not the error row.
    forget_orphan_in(&mut tx, id).await?;
    assert!(load_in(&mut tx, id).await?.is_none());
    let processing: i32 =
        sqlx::query_scalar("SELECT processing FROM media_attachments WHERE id=$1")
            .bind(id.media_id)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(processing, 3);
    tx.rollback().await?;
    Ok(())
}
