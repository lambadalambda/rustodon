//! Focused credentials HTTP regression matrix, run only in the disposable fixture gate.
//!
//! Oracle: pinned Mastodon 4.6.5 `credentials_spec.rb`, read-only extraction at
//! /Users/lainsoykaf/repos/rustodon/.local-instance/audit-reference/remaining/spec/requests/api/v1/accounts/credentials_spec.rb.
//! The upstream request establishes GIF avatar + JPEG header and overlong-note 422;
//! preservation/slot isolation/rollback below extend those cases. No 4.7 oracle,
//! production preparation helpers, or cross-implementation encoded-byte equality.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, GenericImageView, ImageFormat};
use reqwest::Method;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HOST, HeaderMap, HeaderValue};
use rustodon::mastodon::{WriteRepository, random_auth_token};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};
use url::Url;

use super::artifacts::MediaSnapshot;
use super::comparison::CapturedResponse;
use super::harness::{RequestSpec, send_single};
use super::safety::DifferentialConfig;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const ORIGIN: &str = "fixture-v4-6-5.rustodon.invalid";
const BOUNDARY: &str = "rustodon-mixed-profile-boundary";
const UPDATE: &str = "/api/v1/accounts/update_credentials";
const VERIFY: &str = "/api/v1/accounts/verify_credentials";
const GIF: &[u8] = include_bytes!("../fixtures/media/avatar.gif");
const JPEG: &[u8] = include_bytes!("../fixtures/media/attachment.jpg");
const PNG: &[u8] = include_bytes!("../fixtures/media/emojo.png");

type Upload<'a> = (&'a str, &'a str, &'a str, &'a [u8]);

struct Fixture<'a> {
    url: &'a Url,
    media: &'a Path,
    connection: PgConnection,
    account_id: i64,
    token: String,
    phase: String,
    phase_started: Instant,
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedFrame {
    dimensions: (u32, u32),
    delay: (u32, u32),
    pixels_sha256: Vec<u8>,
}

#[derive(PartialEq, Eq)]
struct StoredImage {
    bytes: Vec<u8>,
    frames: Vec<DecodedFrame>,
}

impl std::fmt::Debug for StoredImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredImage")
            .field("format", &image::guess_format(&self.bytes).ok())
            .field("byte_count", &self.bytes.len())
            .field("sha256", &format!("{:x}", Sha256::digest(&self.bytes)))
            .field("frame_count", &self.frames.len())
            .field(
                "first_frame_dimensions",
                &self.frames.first().map(|frame| frame.dimensions),
            )
            .finish()
    }
}

#[test]
fn stored_image_debug_is_compact_and_equality_remains_exact() {
    let sample = || StoredImage {
        bytes: vec![241; 32_768],
        frames: (0..256)
            .map(|_| DecodedFrame {
                dimensions: (400, 400),
                delay: (50, 1),
                pixels_sha256: vec![42; 32],
            })
            .collect(),
    };
    let image = sample();
    let debug = format!("{image:?}");
    assert!(
        debug.len() < 512,
        "image diagnostic must not dump byte/frame arrays"
    );
    assert!(debug.contains(&format!("{:x}", Sha256::digest(&image.bytes))));
    let mut changed = sample();
    assert_eq!(image, changed);
    changed.bytes[0] ^= 1;
    assert_ne!(image, changed, "encoded-byte equality must stay exact");
    changed = sample();
    changed.frames[0].delay = (60, 1);
    assert_ne!(image, changed, "decoded-frame equality must stay exact");
}

#[derive(Debug, PartialEq, Eq)]
struct Slot {
    metadata: BTreeMap<String, Value>,
    urls: [String; 2],
    files: BTreeMap<String, StoredImage>,
}

/// Parent registers this runner with the existing production-router fixture gate.
/// A unique account avoids touching shared seeded users. The marked disposable
/// database/media workspace owns its lifetime (including failure artifacts).
#[allow(clippy::too_many_lines)]
pub(crate) async fn run_mixed_profile_media_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> TestResult {
    let setup_started = Instant::now();
    eprintln!(
        "mixed-profile phase=setup start runtime={:?}",
        tokio::runtime::Handle::current().runtime_flavor()
    );
    config.validate_database_comments().await?;
    let writer = WriteRepository::connect(
        config
            .rust_write_database
            .as_ref()
            .ok_or("missing fixture writer")?
            .url(),
    )
    .await?;
    let owner = config
        .rust_owner_database
        .as_ref()
        .ok_or("missing fixture owner")?;
    let username = format!("mixed{}", random_auth_token(8).replace('-', "_"));
    let user = writer
        .create_local_user(
            &format!("{username}@fixture.invalid"),
            &username,
            "mixed-profile-fixture-password",
        )
        .await?;
    let mut connection = PgConnection::connect(owner.url()).await?;
    let token = random_auth_token(32);
    sqlx::query(
        "INSERT INTO oauth_access_tokens \
         (created_at, resource_owner_id, scopes, token) \
         VALUES (clock_timestamp(), $1, 'read:accounts write:accounts', $2)",
    )
    .bind(user.user_id)
    .bind(&token)
    .execute(&mut connection)
    .await?;
    let mut fixture = Fixture {
        url: rust_url,
        media: &config.rust_media,
        connection,
        account_id: user.account_id,
        token,
        phase: "setup".to_owned(),
        phase_started: setup_started,
    };
    fixture.begin_phase("initial credentials");
    let empty = fixture.verify().await?;
    fixture.begin_phase("mixed GIF-avatar/JPEG-header upload");
    let initial = fixture
        .patch(
            &[
                ("display_name", "Alice Isn't Dead"),
                ("note", "Hello!"),
                ("locked", "false"),
                ("discoverable", "true"),
                ("indexable", "true"),
                ("avatar_description", "Mixed GIF avatar"),
                ("header_description", "Mixed JPEG header"),
                ("source[privacy]", "unlisted"),
                ("source[sensitive]", "true"),
            ],
            &[
                ("avatar", "avatar.gif", "image/gif", GIF),
                ("header", "attachment.jpg", "image/jpeg", JPEG),
            ],
            200,
        )
        .await?;
    assert_eq!(initial["display_name"], "Alice Isn't Dead");
    assert_eq!(initial["source"]["note"], "Hello!");
    assert_eq!(initial["locked"], false);
    assert_eq!(initial["source"]["discoverable"], true);
    assert_eq!(initial["source"]["indexable"], true);
    assert_eq!(initial["source"]["privacy"], "unlisted");
    assert_eq!(initial["source"]["sensitive"], true);
    assert_eq!(initial, fixture.verify().await?);
    let row = fixture.account().await?;
    assert_eq!(row["display_name"], "Alice Isn't Dead");
    assert_eq!(row["note"], "Hello!");
    let avatar = fixture.slot(&initial, "avatar").await?;
    let header = fixture.slot(&initial, "header").await?;
    assert_uploaded(&avatar, "image/gif", (400, 400), GIF)?;
    assert_uploaded(&header, "image/jpeg", (600, 400), JPEG)?;
    assert_eq!(initial["avatar_description"], "Mixed GIF avatar");
    assert_eq!(initial["header_description"], "Mixed JPEG header");

    fixture.begin_phase("text-only preservation");
    let before_text = MediaSnapshot::capture(fixture.media)?;
    let text = fixture
        .patch(
            &[("display_name", "Text only"), ("note", "Media stays put")],
            &[],
            200,
        )
        .await?;
    assert_eq!(text["display_name"], "Text only");
    assert_eq!(text["source"]["note"], "Media stays put");
    assert_eq!(text, fixture.verify().await?);
    let row = fixture.account().await?;
    assert_eq!(row["display_name"], "Text only");
    assert_eq!(row["note"], "Media stays put");
    assert_eq!(avatar, fixture.slot(&text, "avatar").await?);
    assert_eq!(header, fixture.slot(&text, "header").await?);
    assert_eq!(before_text, MediaSnapshot::capture(fixture.media)?);

    rejected_multipart_is_atomic(&mut fixture).await?;

    // Replace each slot while the other remains populated; no duplicate CRUD setup.
    let mut current = text;
    for (slot, opposite, upload, mime, dimensions) in [
        (
            "avatar",
            "header",
            ("avatar", "attachment.jpg", "image/jpeg", JPEG),
            "image/jpeg",
            (400, 400),
        ),
        (
            "header",
            "avatar",
            ("header", "emojo.png", "image/png", PNG),
            "image/png",
            image::load_from_memory(PNG)?.dimensions(),
        ),
    ] {
        fixture.begin_phase(format!("replace {slot}"));
        let old = fixture.slot(&current, slot).await?;
        let preserved = fixture.slot(&current, opposite).await?;
        let files_before = MediaSnapshot::capture(fixture.media)?;
        current = fixture.patch(&[], &[upload], 200).await?;
        assert_eq!(current, fixture.verify().await?);
        let replacement = fixture.slot(&current, slot).await?;
        assert_uploaded(&replacement, mime, dimensions, upload.3)?;
        assert_ne!(
            old.urls[0], replacement.urls[0],
            "{slot} replacement was ignored"
        );
        assert_eq!(preserved, fixture.slot(&current, opposite).await?);
        fixture.assert_old_files_gone(&old).await?;
        assert_only_slot_changed(
            files_before,
            MediaSnapshot::capture(fixture.media)?,
            &old,
            &replacement,
        );
    }

    // JSON null is the credentials removal form; absence was exercised above.
    for (slot, opposite) in [("header", "avatar"), ("avatar", "header")] {
        fixture.begin_phase(format!("remove {slot}"));
        let old = fixture.slot(&current, slot).await?;
        let preserved = fixture.slot(&current, opposite).await?;
        assert!(
            !preserved.files.is_empty(),
            "removal must preserve a populated {opposite}"
        );
        let files_before = MediaSnapshot::capture(fixture.media)?;
        let mut body = serde_json::Map::new();
        body.insert(slot.to_owned(), Value::Null);
        let response = fixture
            .request(
                Method::PATCH,
                UPDATE,
                "application/json",
                serde_json::to_vec(&body)?,
            )
            .await?;
        current = json_response(&response, 200)?;
        assert_eq!(current, fixture.verify().await?);
        let removed = fixture.slot(&current, slot).await?;
        for field in [
            "content_type",
            "file_name",
            "file_size",
            "storage_schema_version",
            "updated_at",
        ] {
            assert_eq!(removed.metadata[&format!("{slot}_{field}")], Value::Null);
        }
        assert!(removed.files.is_empty());
        assert_eq!(current[slot], empty[slot]);
        assert_eq!(
            current[format!("{slot}_static")],
            empty[format!("{slot}_static")]
        );
        assert_eq!(preserved, fixture.slot(&current, opposite).await?);
        fixture.assert_old_files_gone(&old).await?;
        assert_only_slot_changed(
            files_before,
            MediaSnapshot::capture(fixture.media)?,
            &old,
            &removed,
        );
        if slot == "header" {
            fixture.begin_phase("restore header before avatar removal");
            // Restore only the removed slot so avatar removal also has a live
            // opposite image to preserve, rather than testing an empty header.
            current = fixture
                .patch(&[], &[("header", "emojo.png", "image/png", PNG)], 200)
                .await?;
            assert_eq!(current, fixture.verify().await?);
            let restored = fixture.slot(&current, "header").await?;
            assert_uploaded(
                &restored,
                "image/png",
                image::load_from_memory(PNG)?.dimensions(),
                PNG,
            )?;
            assert_eq!(preserved, fixture.slot(&current, "avatar").await?);
        }
    }
    eprintln!(
        "mixed-profile phase={} done elapsed_ms={}",
        fixture.phase,
        fixture.phase_started.elapsed().as_millis()
    );
    eprintln!(
        "mixed-profile complete total_elapsed_ms={}",
        setup_started.elapsed().as_millis()
    );
    Ok(())
}

async fn rejected_multipart_is_atomic(fixture: &mut Fixture<'_>) -> TestResult {
    let long_note = "a".repeat(1000);
    // Different bytes force new paths during validation rollback; same bytes
    // exercise protection of already-installed paths during rollback cleanup.
    for (label, uploads, note) in [
        (
            "valid avatar / invalid header",
            [
                ("avatar", "attachment.jpg", "image/jpeg", JPEG),
                (
                    "header",
                    "bad.jpg",
                    "image/jpeg",
                    b"not an image".as_slice(),
                ),
            ],
            "Rejected",
        ),
        (
            "invalid avatar / valid header",
            [
                ("avatar", "bad.gif", "image/gif", b"not an image".as_slice()),
                ("header", "emojo.png", "image/png", PNG),
            ],
            "Rejected",
        ),
        (
            "new images / invalid note",
            [
                ("avatar", "attachment.jpg", "image/jpeg", JPEG),
                ("header", "emojo.png", "image/png", PNG),
            ],
            long_note.as_str(),
        ),
        (
            "existing images / invalid note",
            [
                ("avatar", "avatar.gif", "image/gif", GIF),
                ("header", "attachment.jpg", "image/jpeg", JPEG),
            ],
            long_note.as_str(),
        ),
    ] {
        fixture.begin_phase(format!("reject {label}"));
        let api_before = fixture.verify().await?;
        let account_before = fixture.account().await?;
        let user_before = fixture.user().await?;
        let files_before = MediaSnapshot::capture(fixture.media)?;
        let avatar_before = fixture.slot(&api_before, "avatar").await?;
        let header_before = fixture.slot(&api_before, "header").await?;
        let response = fixture
            .patch(
                &[
                    ("display_name", "Must not persist"),
                    ("note", note),
                    ("avatar_description", "Rejected avatar"),
                    ("header_description", "Rejected header"),
                    ("source[privacy]", "private"),
                    ("source[sensitive]", "false"),
                ],
                &uploads,
                422,
            )
            .await?;
        assert!(
            response["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "{label}: missing error"
        );
        // Do not print complete account/user rows: they include fixture secrets.
        assert!(
            account_before == fixture.account().await?,
            "{label}: account partially changed"
        );
        assert!(
            user_before == fixture.user().await?,
            "{label}: user settings partially changed"
        );
        assert_eq!(
            files_before,
            MediaSnapshot::capture(fixture.media)?,
            "{label}: file mutation"
        );
        let api_after = fixture.verify().await?;
        assert_eq!(api_before, api_after, "{label}: API mutation");
        assert_eq!(
            avatar_before,
            fixture.slot(&api_after, "avatar").await?,
            "{label}: avatar mutation"
        );
        assert_eq!(
            header_before,
            fixture.slot(&api_after, "header").await?,
            "{label}: header mutation"
        );
    }
    Ok(())
}

impl Fixture<'_> {
    fn begin_phase(&mut self, phase: impl Into<String>) {
        eprintln!(
            "mixed-profile phase={} done elapsed_ms={}",
            self.phase,
            self.phase_started.elapsed().as_millis()
        );
        self.phase = phase.into();
        self.phase_started = Instant::now();
        eprintln!("mixed-profile phase={} start", self.phase);
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> TestResult<CapturedResponse> {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static(ORIGIN));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_str(content_type)?);
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        let request = RequestSpec::new(method.clone(), path, None, headers, body)?;
        let started = Instant::now();
        eprintln!(
            "mixed-profile phase={} request={method} {path} start",
            self.phase
        );
        let result = send_single(self.url, &request, "Rust mixed profile fixture").await;
        let elapsed_ms = started.elapsed().as_millis();
        match result {
            Ok(response) => {
                eprintln!(
                    "mixed-profile phase={} request={method} {path} status={} elapsed_ms={elapsed_ms}",
                    self.phase, response.status
                );
                Ok(response)
            }
            Err(error) => {
                // Only test-owned labels, method/path, elapsed time, and the
                // transport error: never multipart bodies, headers, or tokens.
                let message = format!(
                    "mixed-profile phase={} request={method} {path} failed elapsed_ms={elapsed_ms}: {error}",
                    self.phase
                );
                eprintln!("{message}");
                Err(message.into())
            }
        }
    }

    async fn patch(
        &self,
        fields: &[(&str, &str)],
        uploads: &[Upload<'_>],
        status: u16,
    ) -> TestResult<Value> {
        let response = self
            .request(
                Method::PATCH,
                UPDATE,
                &format!("multipart/form-data; boundary={BOUNDARY}"),
                multipart(fields, uploads),
            )
            .await?;
        json_response(&response, status)
    }

    async fn verify(&self) -> TestResult<Value> {
        json_response(
            &self
                .request(Method::GET, VERIFY, "application/json", Vec::new())
                .await?,
            200,
        )
    }

    async fn account(&mut self) -> TestResult<Value> {
        Ok(
            sqlx::query_scalar("SELECT to_jsonb(a) FROM accounts a WHERE id = $1")
                .bind(self.account_id)
                .fetch_one(&mut self.connection)
                .await?,
        )
    }

    async fn user(&mut self) -> TestResult<Value> {
        Ok(
            sqlx::query_scalar("SELECT to_jsonb(u) FROM users u WHERE account_id = $1")
                .bind(self.account_id)
                .fetch_one(&mut self.connection)
                .await?,
        )
    }

    async fn slot(&mut self, api: &Value, slot: &str) -> TestResult<Slot> {
        let account = self.account().await?;
        let metadata = account
            .as_object()
            .ok_or("account is not an object")?
            .iter()
            .filter(|(key, _)| key.starts_with(&format!("{slot}_")))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let urls = [
            api[slot].as_str().ok_or("missing media URL")?.to_owned(),
            api[format!("{slot}_static")]
                .as_str()
                .ok_or("missing static URL")?
                .to_owned(),
        ];
        let mut files = BTreeMap::new();
        if !account[format!("{slot}_file_name")].is_null() {
            for raw_url in &urls {
                let url = Url::parse(raw_url)?;
                assert_eq!(url.scheme(), "https");
                assert_eq!(url.host_str(), Some(ORIGIN));
                let path = url
                    .path()
                    .strip_prefix("/system/")
                    .ok_or("media URL outside /system")?;
                assert!(path.starts_with(&format!("accounts/{slot}s/")));
                assert!(
                    Path::new(path)
                        .components()
                        .all(|part| matches!(part, std::path::Component::Normal(_)))
                );
                let bytes = fs::read(self.media.join(path))?;
                let response = self
                    .request(
                        Method::GET,
                        url.path(),
                        "application/octet-stream",
                        Vec::new(),
                    )
                    .await?;
                assert_eq!(response.status, 200, "media GET {path}");
                assert_eq!(response.body, bytes, "HTTP and disk bytes differ: {path}");
                let format = image::guess_format(&bytes)?;
                let mime = match format {
                    ImageFormat::Gif => "image/gif",
                    ImageFormat::Jpeg => "image/jpeg",
                    ImageFormat::Png => "image/png",
                    _ => return Err("unexpected profile image format".into()),
                };
                assert_eq!(
                    response
                        .headers
                        .get(CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok()),
                    Some(mime)
                );
                let frames = decode(&bytes)?;
                assert_eq!(frames, decode(&response.body)?);
                files.insert(path.to_owned(), StoredImage { bytes, frames });
            }
            let original = Url::parse(&urls[0])?;
            let original_path = original
                .path()
                .strip_prefix("/system/")
                .ok_or("missing original path")?;
            assert_eq!(
                account[format!("{slot}_file_name")].as_str(),
                Path::new(original_path)
                    .file_name()
                    .and_then(|name| name.to_str())
            );
            assert_eq!(
                account[format!("{slot}_file_size")],
                json!(files[original_path].bytes.len())
            );
            assert_eq!(account[format!("{slot}_storage_schema_version")], 1);
            assert!(!account[format!("{slot}_updated_at")].is_null());
        }
        Ok(Slot {
            metadata,
            urls,
            files,
        })
    }

    async fn assert_old_files_gone(&self, slot: &Slot) -> TestResult {
        for path in slot.files.keys() {
            assert!(
                !self.media.join(path).exists(),
                "superseded file remains: {path}"
            );
            let response = self
                .request(
                    Method::GET,
                    &format!("/system/{path}"),
                    "application/octet-stream",
                    Vec::new(),
                )
                .await?;
            assert_eq!(response.status, 404, "superseded URL still served: {path}");
        }
        Ok(())
    }
}

fn multipart(fields: &[(&str, &str)], uploads: &[Upload<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    for (name, filename, mime, bytes) in uploads {
        body.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn json_response(response: &CapturedResponse, status: u16) -> TestResult<Value> {
    assert_eq!(
        response.status, status,
        "unexpected JSON response status (body omitted)"
    );
    assert!(
        response
            .headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"))
    );
    Ok(serde_json::from_slice(&response.body)?)
}

fn decode(bytes: &[u8]) -> TestResult<Vec<DecodedFrame>> {
    if image::guess_format(bytes)? == ImageFormat::Gif {
        GifDecoder::new(Cursor::new(bytes))?
            .into_frames()
            .map(|frame| {
                let frame = frame?;
                Ok(DecodedFrame {
                    dimensions: frame.buffer().dimensions(),
                    delay: frame.delay().numer_denom_ms(),
                    pixels_sha256: Sha256::digest(frame.buffer().as_raw()).to_vec(),
                })
            })
            .collect()
    } else {
        let image = image::load_from_memory(bytes)?.to_rgba8();
        Ok(vec![DecodedFrame {
            dimensions: image.dimensions(),
            delay: (0, 1),
            pixels_sha256: Sha256::digest(image.as_raw()).to_vec(),
        }])
    }
}

fn assert_uploaded(slot: &Slot, mime: &str, dimensions: (u32, u32), source: &[u8]) -> TestResult {
    let url = Url::parse(&slot.urls[0])?;
    let path = url
        .path()
        .strip_prefix("/system/")
        .ok_or("missing original path")?;
    let original = &slot.files[path];
    let metadata_type = slot
        .metadata
        .iter()
        .find(|(key, _)| key.ends_with("_content_type"))
        .ok_or("missing content type")?
        .1;
    assert_eq!(metadata_type, mime);
    assert!(
        original
            .frames
            .iter()
            .all(|frame| frame.dimensions == dimensions)
    );
    let source_frames = decode(source)?;
    assert_eq!(
        original.frames.len(),
        source_frames.len(),
        "animation frames lost"
    );
    assert_eq!(
        original
            .frames
            .iter()
            .map(|frame| frame.delay)
            .collect::<Vec<_>>(),
        source_frames
            .iter()
            .map(|frame| frame.delay)
            .collect::<Vec<_>>()
    );
    if mime == "image/gif" {
        assert_eq!(
            image::guess_format(&original.bytes)?,
            ImageFormat::Gif,
            "retain supported GIF, not transcoded video"
        );
        assert_ne!(slot.urls[0], slot.urls[1]);
        assert_eq!(slot.files.len(), 2);
        let static_url = Url::parse(&slot.urls[1])?;
        let still = &slot.files[static_url
            .path()
            .strip_prefix("/system/")
            .ok_or("missing static path")?];
        assert_eq!(image::guess_format(&still.bytes)?, ImageFormat::Png);
        assert_eq!(still.frames.len(), 1);
        assert_eq!(still.frames[0].dimensions, dimensions);
        // Decode/resize the vendored source directly, never call prepare_account_media.
        let first = GifDecoder::new(Cursor::new(source))?
            .into_frames()
            .next()
            .ok_or("empty source GIF")??;
        let expected = image::DynamicImage::ImageRgba8(first.into_buffer())
            .resize_to_fill(
                dimensions.0,
                dimensions.1,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();
        assert_eq!(
            still.frames[0].pixels_sha256,
            Sha256::digest(expected.as_raw()).to_vec()
        );
    } else {
        assert_eq!(slot.urls[0], slot.urls[1]);
        assert_eq!(slot.files.len(), 1);
        let expected = if mime == "image/jpeg" {
            ImageFormat::Jpeg
        } else {
            ImageFormat::Png
        };
        assert_eq!(image::guess_format(&original.bytes)?, expected);
        if mime == "image/png" {
            assert_eq!(
                original.frames, source_frames,
                "unscaled PNG header pixels changed"
            );
        }
    }
    Ok(())
}

fn assert_only_slot_changed(
    mut before: MediaSnapshot,
    mut after: MediaSnapshot,
    old: &Slot,
    new: &Slot,
) {
    for path in old.files.keys() {
        assert!(before.files.remove(path).is_some());
    }
    for path in new.files.keys() {
        assert!(after.files.remove(path).is_some());
    }
    assert_eq!(
        before, after,
        "slot operation changed unrelated files or leaked a derivative"
    );
}
