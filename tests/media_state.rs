//! Pinned Mastodon 4.6.5 media HTTP state matrix (disposable fixture only).
//!
//! Oracle: read-only extracted spec/requests/api/v1/media_spec.rb:32-68,149-244
//! and app/controllers/api/v1/media_controller.rb:6-7,23-49,63-68 from image
//! sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf.
//! In particular, the controller updates unprocessed attachments and returns 206;
//! failed processing returns 422 *after* owner/unattached lookup. These expectations
//! are not inferred from Rust's repository predicates.
//!
//! Published JPEG metadata with processing 0/1/2/3 exercises pending/in-progress/
//! ready/failed reads and updates, NOT asynchronous processing. V2's synchronous
//! JPEG context is spec/requests/api/v2/media_spec.rb:8-27. Async upload, video,
//! audio, new formats, happy CRUD, and repository concurrency are out of scope for
//! that matrix. The separate test-support `local_upload_http` module adds the bounded
//! real-codec local v2 lifecycle with distinct disposable setup/runtime/writer logins.
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use reqwest::{Client, Method};
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository};
use rustodon::web::{WebState, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use url::Url;

const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const OWNER: i64 = 116_844_606_259_201_001;
const OWNER_TOKEN: &str = "fixture-bearer-token-v4-6-5";
const OTHER_TOKEN: &str = "fixture-bearer-api-moderator-v4-6-5";
const PENDING: i64 = 940_701;
const IN_PROGRESS: i64 = 940_702;
const FAILED: i64 = 940_703;
const READY: i64 = 940_704;
const ATTACHED: i64 = 940_705;
const COMPANION: i64 = 940_706;
const MEDIA_IDS: [i64; 6] = [PENDING, IN_PROGRESS, FAILED, READY, ATTACHED, COMPANION];
const STATUS: i64 = 940_707;
const OLD_DESCRIPTION: &str = "matrix original description";
const NEW_DESCRIPTION: &str = "matrix changed description";
const PROCESSING_ERROR: &str = "Error processing thumbnail for uploaded media";
const ATTACHED_ERROR: &str = "Media attachment is currently used by a status";

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn runtime() -> InstanceRuntimeConfig {
    InstanceRuntimeConfig {
        domain: DOMAIN.to_owned(),
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
        translation_enabled: false,
        limited_federation: false,
        single_user_mode: false,
        terms_of_service_url: None,
        sso_signup_url: None,
        wrapstodon: None,
    }
}

struct Resources {
    root: PathBuf,
    server: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl Drop for Resources {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn jpeg(width: u32, height: u32) -> TestResult<Vec<u8>> {
    let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([35, 95, 155])));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Jpeg)?;
    Ok(bytes.into_inner())
}

async fn seed(pool: &PgPool, root: &Path) -> TestResult<String> {
    let original = jpeg(32, 24)?;
    let preview = jpeg(16, 12)?;
    let mut transaction = pool.begin().await?;
    let old_scope: String =
        sqlx::query_scalar("SELECT scopes FROM oauth_access_tokens WHERE token = $1")
            .bind(OTHER_TOKEN)
            .fetch_one(&mut *transaction)
            .await?;
    // Same write:media scope as the pinned request spec, so 404 cannot be a scope denial.
    sqlx::query("UPDATE oauth_access_tokens SET scopes = 'write:media' WHERE token = $1")
        .bind(OTHER_TOKEN)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO statuses
         (id, account_id, text, spoiler_text, visibility, local, sensitive, reply,
          ordered_media_attachment_ids, created_at, updated_at)
         VALUES ($1, $2, 'media matrix attached status', '', 0, true, false, false,
                 $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(STATUS)
    .bind(OWNER)
    // Deliberately non-ID order; full-row snapshots preserve this exact association order.
    .bind([COMPANION, ATTACHED].as_slice())
    .execute(&mut *transaction)
    .await?;
    for (id, processing) in MEDIA_IDS.into_iter().zip([0_i32, 1, 3, 2, 2, 2]) {
        let status_id = matches!(id, ATTACHED | COMPANION).then_some(STATUS);
        sqlx::query(
            "INSERT INTO media_attachments
             (id, account_id, status_id, type, processing, description, file_content_type,
              file_file_name, file_file_size, file_meta, file_storage_schema_version,
              file_updated_at, created_at, updated_at)
             VALUES ($1, $2, $3, 0, $4, $5, 'image/jpeg', 'state.jpg', $6, $7, 1,
                     clock_timestamp(), clock_timestamp(), clock_timestamp())",
        )
        .bind(id)
        .bind(OWNER)
        .bind(status_id)
        .bind(processing)
        .bind(OLD_DESCRIPTION)
        .bind(i32::try_from(original.len())?)
        .bind(json!({
            "original": {"width": 32, "height": 24, "size": "32x24", "aspect": 32.0 / 24.0},
            "small": {"width": 16, "height": 12, "size": "16x12", "aspect": 16.0 / 12.0},
            "focus": {"x": 0.0, "y": 0.0}
        }))
        .execute(&mut *transaction)
        .await?;
        // Independent Paperclip layout, not a path returned by the subject under test.
        let partition = format!("{id:09}");
        for (style, bytes) in [("original", &original), ("small", &preview)] {
            let directory = root.join(format!(
                "media_attachments/files/{}/{}/{}/{style}",
                &partition[..3],
                &partition[3..6],
                &partition[6..]
            ));
            fs::create_dir_all(&directory)?;
            fs::write(directory.join("state.jpg"), bytes)?;
        }
    }
    transaction.commit().await?;
    Ok(old_scope)
}

fn files(root: &Path, directory: &Path) -> TestResult<BTreeMap<PathBuf, Vec<u8>>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            result.extend(files(root, &path)?);
        } else {
            result.insert(path.strip_prefix(root)?.to_path_buf(), fs::read(path)?);
        }
    }
    Ok(result)
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    rows: Value,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

async fn snapshot(pool: &PgPool, root: &Path) -> TestResult<Snapshot> {
    // to_jsonb captures every column, including timestamps, association order, and
    // publication/thumbnail fields. Cleanup snapshots include full rows, not counts;
    // no workers run in this fixture. Rate-limit bookkeeping is intentionally excluded.
    let rows = sqlx::query_scalar(
        "SELECT jsonb_build_object(
           'media', (SELECT COALESCE(jsonb_agg(to_jsonb(m) ORDER BY m.id), '[]'::jsonb)
                     FROM media_attachments m WHERE m.id = ANY($1)),
           'statuses', (SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY s.id), '[]'::jsonb)
                        FROM statuses s WHERE s.id = $2),
           'outbox', (SELECT COALESCE(jsonb_agg(to_jsonb(o) ORDER BY o.id), '[]'::jsonb)
                      FROM rustodon.outbox_events o WHERE o.kind = $3),
           'jobs', (SELECT COALESCE(jsonb_agg(to_jsonb(j) ORDER BY j.id), '[]'::jsonb)
                    FROM rustodon.durable_jobs j WHERE j.kind = $3))",
    )
    .bind(MEDIA_IDS.as_slice())
    .bind(STATUS)
    .bind(rustodon::jobs::LOCAL_MEDIA_CLEANUP_JOB_KIND)
    .fetch_one(pool)
    .await?;
    Ok(Snapshot {
        rows,
        files: files(root, root)?,
    })
}

struct Case {
    name: &'static str,
    method: Method,
    id: i64,
    token: &'static str,
    status: u16,
    error: Option<&'static str>,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    // Controller:23-25,39-48 adds pending + in-progress PUT to spec:32-43's GET.
    for (name, id) in [
        ("owner pending", PENDING),
        ("owner in-progress", IN_PROGRESS),
    ] {
        for method in [Method::GET, Method::PUT] {
            cases.push(Case {
                name,
                method,
                id,
                token: OWNER_TOKEN,
                status: 206,
                error: None,
            });
        }
    }
    // Controller:43-48: authorization/attachment lookup precedes processing failure.
    for (name, id, token, status, error, delete) in [
        (
            "owner failed",
            FAILED,
            OWNER_TOKEN,
            422,
            Some(PROCESSING_ERROR),
            false,
        ),
        ("other ready", READY, OTHER_TOKEN, 404, None, true),
        ("other failed", FAILED, OTHER_TOKEN, 404, None, false),
        (
            "owner attached ready",
            ATTACHED,
            OWNER_TOKEN,
            404,
            None,
            true,
        ),
    ] {
        for method in [Method::GET, Method::PUT] {
            cases.push(Case {
                name,
                method,
                id,
                token,
                status,
                error,
            });
        }
        if delete {
            // Spec:213-244 / controller:28-34: attached owner gets 422, other gets 404.
            let attached = id == ATTACHED;
            cases.push(Case {
                name,
                method: Method::DELETE,
                id,
                token,
                status: if attached { 422 } else { 404 },
                error: attached.then_some(ATTACHED_ERROR),
            });
        }
    }
    cases
}

fn check(failures: &mut Vec<String>, label: &str, actual: &Value, expected: &Value) {
    if actual != expected {
        failures.push(format!("{label}: expected {expected}, got {actual}"));
    }
}

#[allow(clippy::too_many_lines)]
async fn run_matrix(pool: &PgPool, root: &Path, base: &str) -> TestResult<Vec<String>> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut failures = Vec::new();
    for case in cases() {
        let label = format!("{} {} /api/v1/media/{}", case.name, case.method, case.id);
        let before = snapshot(pool, root).await?;
        let mut request = client
            .request(
                case.method.clone(),
                format!("{base}/api/v1/media/{}", case.id),
            )
            .header("host", DOMAIN)
            .bearer_auth(case.token);
        if case.method == Method::PUT {
            request = request
                .header("content-type", "application/json")
                .body(json!({"description": NEW_DESCRIPTION, "focus": "0.25,-0.5"}).to_string());
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let is_json = response.headers().get("content-type").is_some_and(|value| {
            value
                .to_str()
                .is_ok_and(|value| value.starts_with("application/json"))
        });
        let text = response.text().await?;
        check(&mut failures, &label, &json!(status), &json!(case.status));
        if !is_json {
            failures.push(format!("{label}: expected application/json, body {text}"));
        }
        let body: Value = serde_json::from_str(&text)?;
        let after = snapshot(pool, root).await?;
        if case.status != 206 || case.method == Method::GET {
            if before != after {
                failures.push(format!("{label}: rejected/read request mutated rows or files\nbefore: {before:?}\nafter: {after:?}"));
            }
        } else if before.files != after.files {
            failures.push(format!(
                "{label}: metadata-only PUT changed original/preview bytes"
            ));
        }
        // Collect status failures instead of stopping at the first suspected PUT 404:
        // the parent's baseline red must exercise all fourteen requests, including both states.
        if status != case.status {
            continue;
        }
        if let Some(error) = case.error {
            check(&mut failures, &label, &body["error"], &json!(error));
        }
        if case.status == 206 {
            let description = if case.method == Method::PUT {
                NEW_DESCRIPTION
            } else {
                OLD_DESCRIPTION
            };
            check(
                &mut failures,
                &label,
                &body["id"],
                &json!(case.id.to_string()),
            );
            check(&mut failures, &label, &body["type"], &json!("image"));
            check(
                &mut failures,
                &label,
                &body["description"],
                &json!(description),
            );
            if case.method == Method::PUT {
                let focus = json!({"x": 0.25, "y": -0.5});
                check(&mut failures, &label, &body["meta"]["focus"], &focus);
                let row = after.rows["media"]
                    .as_array()
                    .ok_or("missing media snapshot")?
                    .iter()
                    .find(|row| row["id"] == case.id)
                    .ok_or("updated media disappeared")?;
                check(
                    &mut failures,
                    &label,
                    &row["description"],
                    &json!(NEW_DESCRIPTION),
                );
                check(&mut failures, &label, &row["file_meta"]["focus"], &focus);
                let processing = i32::from(case.id != PENDING);
                check(
                    &mut failures,
                    &label,
                    &row["processing"],
                    &json!(processing),
                );
            }
        }
    }
    Ok(failures)
}

#[tokio::test]
#[ignore = "requires disposable reader/writer/owner fixture from tools/mastodon-fixture"]
async fn pinned_media_state_http_matrix() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let reader_url = std::env::var("RUSTODON_MASTODON_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")?;
    let pool = PgPool::connect(&owner_url).await?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    rustodon::operational_schema::migrate(&mut connection).await?;
    let root = std::env::temp_dir().join(format!("rustodon-media-state-{}", std::process::id()));
    fs::create_dir(&root)?;
    let mut resources = Resources {
        root: root.canonicalize()?,
        server: None,
    };
    let old_scope = seed(&pool, &resources.root).await?;
    let state = WebState::new(
        Repository::connect(&reader_url).await?,
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        &resources.root,
        runtime(),
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?
    .with_write_repository(WriteRepository::connect(&writer_url).await?);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    resources.server = Some(tokio::spawn(async move {
        axum::serve(listener, router(state)).await
    }));
    let result = run_matrix(&pool, &resources.root, &base).await;
    sqlx::query("DELETE FROM media_attachments WHERE id = ANY($1)")
        .bind(MEDIA_IDS.as_slice())
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(STATUS)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE oauth_access_tokens SET scopes = $1 WHERE token = $2")
        .bind(old_scope)
        .bind(OTHER_TOKEN)
        .execute(&pool)
        .await?;
    let failures = result?;
    assert!(
        failures.is_empty(),
        "pinned 4.6.5 media matrix:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[cfg(feature = "test-support")]
#[path = "support/local_upload_http.rs"]
mod local_upload_http;
