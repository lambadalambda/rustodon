//! Representation-boundary regressions for already-cached remote media.
//! Ordinary helper tests need no DB, HTTP server, upstream fixtures, or transcoder.
//! The ignored HTTP test checks authorization using the disposable schema fixture.
use super::*;
use std::fs;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};

use image::{DynamicImage, GenericImageView, ImageFormat, Rgb, RgbImage};
use sqlx::postgres::PgPoolOptions;

use crate::mastodon::RawI32;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MEDIA_ID: i64 = 12_001;
const CACHE_DIRECTORY: &str = "cache/media_attachments/files/000/012/001";

struct CachedMediaFixture {
    state: WebState,
    root: PathBuf,
}

impl CachedMediaFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rustodon-cached-media-mime-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create isolated media root");
        let root = root.canonicalize().unwrap();
        // The response helper never uses the repository. A lazy pool needs no DB.
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let runtime = InstanceRuntimeConfig {
            domain: "cached-media.invalid".to_owned(),
            version: "test".to_owned(),
            source_url: String::new(),
            streaming_api: String::new(),
            vapid_public_key: None,
            thumbnail_url: String::new(),
            thumbnail_description: String::new(),
            thumbnail_blurhash: None,
            thumbnail_versions: None,
            icons: Vec::new(),
            languages: Vec::new(),
            active_month: 0,
            active_halfyear: 0,
            translation_enabled: false,
            limited_federation: false,
            single_user_mode: false,
            terms_of_service_url: None,
            sso_signup_url: None,
            wrapstodon: None,
        };
        let state = WebState::new(
            Repository::from_pool(pool),
            Url::parse("https://cached-media.invalid/").unwrap(),
            "cached-media.invalid",
            "/system",
            &root,
            runtime,
            Vec::new(),
            vec!["cached-media.invalid".to_owned()],
        )
        .expect("construct state with a secure media root");
        Self { state, root }
    }

    fn write(&self, style: &str, file_name: &str, bytes: &[u8]) -> PathBuf {
        // Deliberately independent of PaperclipMetadata::relative_path: assert
        // the production helper opens the expected style, not its own oracle.
        let path = self.root.join(CACHE_DIRECTORY).join(style).join(file_name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for CachedMediaFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn cached_remote_media_response(
    state: &WebState,
    media: &MediaAttachment,
    small: bool,
) -> Option<Response<Body>> {
    super::cached_remote_media_response(state, media, small, &Method::GET, &HeaderMap::new())
}

fn media(file_name: &str, content_type: &str) -> MediaAttachment {
    let now = Utc::now().naive_utc();
    MediaAttachment {
        id: MEDIA_ID,
        account_id: Some(12_002),
        status_id: Some(12_003),
        media_type: RawI32(i32::from(content_type.starts_with("video/")) * 2),
        processing: Some(RawI32(2)),
        description: None,
        remote_url: format!("https://remote.invalid/{file_name}"),
        file_content_type: Some(content_type.to_owned()),
        file_file_name: Some(file_name.to_owned()),
        // Deliberately not authoritative for the bytes of a cached derivative.
        file_file_size: Some(1),
        file_meta: None,
        file_storage_schema_version: Some(1),
        file_updated_at: None,
        scheduled_status_id: None,
        shortcode: None,
        // A separate thumbnail's metadata must not override the file's small style.
        thumbnail_content_type: Some("image/webp".to_owned()),
        thumbnail_file_name: Some("separate.webp".to_owned()),
        thumbnail_file_size: None,
        thumbnail_remote_url: Some("https://remote.invalid/separate.webp".to_owned()),
        thumbnail_storage_schema_version: Some(1),
        thumbnail_updated_at: None,
        blurhash: None,
        created_at: now,
        updated_at: now,
    }
}

fn encoded_image(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([35, 95, 155])));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

async fn response_bytes(
    fixture: &CachedMediaFixture,
    media: &MediaAttachment,
    small: bool,
    expected_mime: &str,
) -> Vec<u8> {
    let response = cached_remote_media_response(&fixture.state, media, small)
        .expect("the expected cached style exists and is within the byte limit");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], expected_mime);
    assert_eq!(response.headers()[CACHE_CONTROL], "private, no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    axum::body::to_bytes(response.into_body(), MEDIA_PROXY_MAX_RESPONSE_BYTES)
        .await
        .unwrap()
        .to_vec()
}

fn assert_decodes(bytes: &[u8], format: ImageFormat, dimensions: (u32, u32)) {
    assert_eq!(image::guess_format(bytes).unwrap(), format);
    let decoded = image::load_from_memory_with_format(bytes, format)
        .expect("response body must actually decode as the advertised image type");
    assert_eq!(decoded.dimensions(), dimensions);
}

async fn assert_small_representation(
    original_name: &str,
    original_mime: &str,
    small_name: &str,
    small_mime: &str,
    format: ImageFormat,
) {
    let fixture = CachedMediaFixture::new();
    let media = media(original_name, original_mime);
    let original_metadata = media.clone();
    // Opaque sentinels make accidental original-file selection observable.
    // Video/HEIF/AVIF decoding is not supported by our image dependency and is
    // not required to serve existing cached derivatives (no transcoding here).
    fixture.write(
        "original",
        original_name,
        b"original must not be served as small",
    );
    let expected = encoded_image(format, 2, 1);
    fixture.write("small", small_name, &expected);
    let actual = response_bytes(&fixture, &media, true, small_mime).await;
    assert_eq!(actual, expected);
    assert_decodes(&actual, format, (2, 1));
    assert_eq!(
        media, original_metadata,
        "serving small must not rewrite original metadata"
    );
}

#[tokio::test]
async fn mp4_small_is_decodable_png_not_original_video_mime() {
    assert_small_representation(
        "movie.mp4",
        "video/mp4",
        "movie.png",
        "image/png",
        ImageFormat::Png,
    )
    .await;
}

#[tokio::test]
async fn converted_image_small_is_decodable_jpeg_not_original_image_mime() {
    for (file_name, content_type) in [
        ("photo.heic", "image/heic"),
        ("photo.heif", "image/heif"),
        ("photo.avif", "image/avif"),
    ] {
        assert_small_representation(
            file_name,
            content_type,
            "photo.jpeg",
            "image/jpeg",
            ImageFormat::Jpeg,
        )
        .await;
    }
}

#[tokio::test]
async fn gif_original_and_png_small_have_distinct_decodable_representations() {
    let fixture = CachedMediaFixture::new();
    let media = media("animation.gif", "image/gif");
    let original = encoded_image(ImageFormat::Gif, 4, 3);
    let small = encoded_image(ImageFormat::Png, 2, 1);
    fixture.write("original", "animation.gif", &original);
    fixture.write("small", "animation.png", &small);
    for (is_small, mime, format, expected, dimensions) in [
        (false, "image/gif", ImageFormat::Gif, original, (4, 3)),
        (true, "image/png", ImageFormat::Png, small, (2, 1)),
    ] {
        let actual = response_bytes(&fixture, &media, is_small, mime).await;
        assert_eq!(actual, expected);
        assert_decodes(&actual, format, dimensions);
    }
}

#[tokio::test]
async fn unchanged_image_types_keep_mime_and_serve_the_requested_decodable_style() {
    for (file_name, mime, format) in [
        ("photo.png", "image/png", ImageFormat::Png),
        ("photo.jpg", "image/jpeg", ImageFormat::Jpeg),
        ("photo.webp", "image/webp", ImageFormat::WebP),
    ] {
        let fixture = CachedMediaFixture::new();
        let media = media(file_name, mime);
        let original = encoded_image(format, 4, 3);
        let small = encoded_image(format, 2, 1);
        fixture.write("original", file_name, &original);
        fixture.write("small", file_name, &small);
        for (is_small, expected, dimensions) in [(false, original, (4, 3)), (true, small, (2, 1))] {
            let actual = response_bytes(&fixture, &media, is_small, mime).await;
            assert_eq!(actual, expected);
            assert_decodes(&actual, format, dimensions);
        }
    }
}

#[tokio::test]
async fn video_and_converted_originals_remain_opaque_passthrough_with_original_mime() {
    let fixture = CachedMediaFixture::new();
    // This explicitly tests byte-preserving passthrough, not video/HEIF decoding.
    for (file_name, mime, small_name) in [
        ("movie.mp4", "video/mp4", "movie.png"),
        ("photo.heic", "image/heic", "photo.jpeg"),
        ("photo.heif", "image/heif", "photo.jpeg"),
        ("photo.avif", "image/avif", "photo.jpeg"),
    ] {
        let media = media(file_name, mime);
        let expected = format!("opaque original payload for {mime}").into_bytes();
        fixture.write("original", file_name, &expected);
        fixture.write("small", small_name, b"small must not be served as original");
        assert_eq!(
            response_bytes(&fixture, &media, false, mime).await,
            expected
        );
    }
}

#[tokio::test]
async fn both_styles_enforce_actual_cached_bytes_at_the_response_size_limit() {
    let fixture = CachedMediaFixture::new();
    let media = media("photo.png", "image/png");
    for (style, small) in [("original", false), ("small", true)] {
        let mut expected = encoded_image(ImageFormat::Png, 2, 1);
        // Valid PNG with trailing padding: the limit concerns the stored bytes,
        // not the dimensions, decoded allocation, or original file_file_size.
        expected.resize(MEDIA_PROXY_MAX_RESPONSE_BYTES, 0);
        let path = fixture.write(style, "photo.png", &expected);
        let actual = response_bytes(&fixture, &media, small, "image/png").await;
        assert_eq!(actual, expected);
        assert_decodes(&actual, ImageFormat::Png, (2, 1));
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(u64::try_from(MEDIA_PROXY_MAX_RESPONSE_BYTES).unwrap() + 1)
            .unwrap();
        assert!(cached_remote_media_response(&fixture.state, &media, small).is_none());
    }
}

#[tokio::test]
async fn missing_small_is_a_cache_miss_not_an_original_response() {
    let fixture = CachedMediaFixture::new();
    let media = media("photo.png", "image/png");
    assert!(cached_remote_media_response(&fixture.state, &media, false).is_none());
    fixture.write(
        "original",
        "photo.png",
        &encoded_image(ImageFormat::Png, 4, 3),
    );
    assert!(cached_remote_media_response(&fixture.state, &media, true).is_none());
}

#[tokio::test]
#[ignore = "requires disposable schema-read fixture owner/reader URLs; media_proxy selector"]
#[allow(clippy::too_many_lines)]
async fn cached_private_media_http_requires_status_access_even_when_files_exist()
-> Result<(), Box<dyn std::error::Error>> {
    const STATUS_ID: i64 = 12_003;
    const AUTHOR_ID: i64 = 116_844_606_259_202_001; // remote Bob
    const FOLLOWER: &str = "fixture-bearer-read-statuses-v4-6-5"; // Alice follows Bob
    const OUTSIDER: &str = "fixture-bearer-matrix-viewer-v4-6-5"; // does not follow Bob

    let owner = PgPool::connect(&std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let reader = PgPool::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let runtime_identity: (String, bool, bool) = sqlx::query_as(
        "SELECT current_user::text, rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls, has_table_privilege(current_user, 'media_attachments', 'UPDATE') FROM pg_roles WHERE rolname=current_user",
    ).fetch_one(&reader).await?;
    let owner_identity: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&owner)
        .await?;
    assert_ne!(runtime_identity.0, owner_identity);
    assert!(
        !runtime_identity.1 && !runtime_identity.2,
        "HTTP must use the restricted reader"
    );
    // Only add test-owned rows; don't change visibility or relationships on seed rows.
    let mut setup = owner.begin().await?;
    sqlx::query(
        "INSERT INTO statuses (id, account_id, text, spoiler_text, visibility, local, uri, \
         sensitive, reply, created_at, updated_at) \
         VALUES ($1, $2, 'Cached private media HTTP regression', '', 2, false, \
         'https://remote.fixture.invalid/statuses/cached-media-mime-test', false, false, now(), now())",
    )
    .bind(STATUS_ID)
    .bind(AUTHOR_ID)
    .execute(&mut *setup)
    .await?;
    sqlx::query(
        "INSERT INTO media_attachments (id, account_id, status_id, type, processing, \
         remote_url, file_file_name, file_content_type, file_storage_schema_version, \
         created_at, updated_at) \
         VALUES ($1, $2, $3, 0, 2, 'https://remote.fixture.invalid/animation.gif', \
         'animation.gif', 'image/gif', 1, now(), now())",
    )
    .bind(MEDIA_ID)
    .bind(AUTHOR_ID)
    .bind(STATUS_ID)
    .execute(&mut *setup)
    .await?;
    setup.commit().await?;

    let fixture = CachedMediaFixture::new();
    let original = encoded_image(ImageFormat::Gif, 4, 3);
    let small = encoded_image(ImageFormat::Png, 2, 1);
    let original_path = fixture.write("original", "animation.gif", &original);
    let small_path = fixture.write("small", "animation.png", &small);
    let mut state = fixture.state.clone();
    state.authenticator = BearerAuthenticator::new(repository.clone());
    state.repository = repository;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(StdDuration::from_secs(10))
        .build()?;

    for (style, mime, format, expected, dimensions) in [
        ("original", "image/gif", ImageFormat::Gif, original, (4, 3)),
        ("small", "image/png", ImageFormat::Png, small, (2, 1)),
    ] {
        // Also deny after a successful authorized request to the identical URL.
        for token in [None, Some(OUTSIDER), Some(FOLLOWER), None] {
            assert!(original_path.is_file() && small_path.is_file());
            let mut request = client
                .get(format!("{base}/media_proxy/{MEDIA_ID}/{style}"))
                .header("host", "cached-media.invalid");
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await?;
            if token == Some(FOLLOWER) {
                assert_eq!(response.status(), StatusCode::OK, "{style}");
                assert_eq!(response.headers()[CONTENT_TYPE], mime);
                assert_eq!(response.headers()[CACHE_CONTROL], "private, no-store");
                assert_eq!(response.headers()["x-content-type-options"], "nosniff");
                let bytes = response.bytes().await?;
                assert_eq!(bytes.as_ref(), expected.as_slice());
                assert_decodes(&bytes, format, dimensions);
            } else {
                assert_eq!(
                    response.status(),
                    StatusCode::NOT_FOUND,
                    "{style} {token:?}"
                );
                assert_eq!(
                    response.headers()[CONTENT_TYPE],
                    "application/json; charset=utf-8"
                );
                let bytes = response.bytes().await?;
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&bytes)?,
                    serde_json::json!({"error": "Not Found"})
                );
            }
        }
    }
    // Normalized rows retain source URLs that no longer describe their cached bytes.
    for (kind, mime, name, source, preview) in [
        (
            2,
            "video/mp4",
            "normalized.mp4",
            "source.mov",
            Some("normalized.png"),
        ),
        (
            0,
            "image/jpeg",
            "normalized.jpeg",
            "source.heic",
            Some("normalized.jpeg"),
        ),
        (4, "audio/mpeg", "normalized.mp3", "source.wav", None),
    ] {
        sqlx::query("UPDATE media_attachments SET type=$2, file_content_type=$3, file_file_name=$4, remote_url=$5 WHERE id=$1")
            .bind(MEDIA_ID).bind(kind).bind(mime).bind(name)
            .bind(format!("https://remote.fixture.invalid/{source}"))
            .execute(&owner).await?;
        let original = if kind == 0 {
            encoded_image(ImageFormat::Jpeg, 4, 3)
        } else {
            b"normalized original bytes".to_vec()
        };
        fixture.write("original", name, &original);
        let preview_bytes = if kind == 0 {
            encoded_image(ImageFormat::Jpeg, 2, 1)
        } else {
            encoded_image(ImageFormat::Png, 2, 1)
        };
        if let Some(name) = preview {
            fixture.write("small", name, &preview_bytes);
        }
        let response = client
            .get(format!("{base}/media_proxy/{MEDIA_ID}/original"))
            .header("host", "cached-media.invalid")
            .bearer_auth(FOLLOWER)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], mime);
        assert_eq!(response.bytes().await?.as_ref(), original.as_slice());
        let url = format!("{base}/media_proxy/{MEDIA_ID}/small");
        let response = client
            .get(&url)
            .header("host", "cached-media.invalid")
            .bearer_auth(FOLLOWER)
            .send()
            .await?;
        if let Some(preview_name) = preview {
            let preview_mime = if kind == 0 { "image/jpeg" } else { "image/png" };
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[CONTENT_TYPE], preview_mime);
            let bytes = response.bytes().await?;
            assert_eq!(bytes.as_ref(), preview_bytes.as_slice());
            assert_decodes(
                &bytes,
                if kind == 0 {
                    ImageFormat::Jpeg
                } else {
                    ImageFormat::Png
                },
                (2, 1),
            );
            // Exact serialized local URL, not just the proxy alias.
            let local = format!("{base}/system/{CACHE_DIRECTORY}/small/{preview_name}");
            for (method, target) in [
                (Method::GET, &local),
                (Method::HEAD, &local),
                (Method::GET, &url),
                (Method::HEAD, &url),
            ] {
                let response = client
                    .request(method.clone(), target)
                    .header("host", "cached-media.invalid")
                    .header("range", "bytes=0-7")
                    .bearer_auth(FOLLOWER)
                    .send()
                    .await?;
                assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
                assert_eq!(response.headers()[CONTENT_TYPE], preview_mime);
                assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
                assert_eq!(
                    response.bytes().await?.as_ref(),
                    if method == Method::HEAD {
                        &[]
                    } else {
                        &preview_bytes[..8]
                    }
                );
            }
            let denied = client
                .get(&local)
                .header("host", "cached-media.invalid")
                .bearer_auth(OUTSIDER)
                .header("range", "bytes=0-7")
                .send()
                .await?;
            assert_eq!(denied.status(), StatusCode::NOT_FOUND);
            assert_eq!(denied.headers()[CACHE_CONTROL], PRIVATE_CACHE);
        } else {
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        let denied = client
            .get(&url)
            .header("host", "cached-media.invalid")
            .bearer_auth(OUTSIDER)
            .send()
            .await?;
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
        assert_eq!(denied.headers()[CACHE_CONTROL], PRIVATE_CACHE);
    }
    for processing in [0, 3] {
        sqlx::query("UPDATE media_attachments SET processing=$2, file_file_name=NULL, type=2, file_content_type='video/mp4', thumbnail_remote_url=NULL WHERE id=$1")
            .bind(MEDIA_ID).bind(processing).execute(&owner).await?;
        let response = client
            .get(format!("{base}/media_proxy/{MEDIA_ID}/small"))
            .header("host", "cached-media.invalid")
            .bearer_auth(FOLLOWER)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
    }
    assert_explicit_thumbnail_storage_identity(&owner, &fixture, &client, &base).await?;
    assert_direct_remote_processing_states(&owner, &fixture, &client, &base).await?;
    server.abort();
    let _ = server.await;
    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(MEDIA_ID)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(STATUS_ID)
        .execute(&owner)
        .await?;
    Ok(())
}

#[tokio::test]
async fn rich_cached_original_bounds_do_not_widen_image_or_preview_limits() {
    let fixture = CachedMediaFixture::new();
    for (name, mime) in [("movie.mp4", "video/mp4"), ("audio.mp3", "audio/mpeg")] {
        let media = media(name, mime);
        let path = fixture.write("original", name, b"bounded cached representation");
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_len(crate::media::AUDIO_VIDEO_SIZE_LIMIT as u64)
            .unwrap();
        assert!(cached_remote_media_response(&fixture.state, &media, false).is_some());
        file.set_len(crate::media::AUDIO_VIDEO_SIZE_LIMIT as u64 + 1)
            .unwrap();
        assert!(cached_remote_media_response(&fixture.state, &media, false).is_none());
    }
}

#[tokio::test]
async fn pending_failed_cached_rich_bytes_are_not_served() {
    let fixture = CachedMediaFixture::new();
    let mut media = media("movie.mp4", "video/mp4");
    fixture.write("original", "movie.mp4", b"stale video");
    fixture.write("small", "movie.png", &encoded_image(ImageFormat::Png, 2, 1));
    for processing in [0, 1, 3] {
        media.processing = Some(RawI32(processing));
        for small in [false, true] {
            assert!(cached_remote_media_response(&fixture.state, &media, small).is_none());
        }
    }
}

#[test]
fn remote_rich_small_never_selects_original_source() {
    for mime in ["video/mp4", "audio/mpeg", "image/heic"] {
        let mut media = media("source", mime);
        media.file_file_name = None;
        media.thumbnail_remote_url = None;
        for state in [0, 1, 3] {
            media.processing = Some(RawI32(state));
            assert_eq!(remote_media_proxy_source(&media, true), None);
        }
    }
}

#[tokio::test]
async fn cached_png_ranges_and_head_share_representation() {
    let fixture = CachedMediaFixture::new();
    let media = media("movie.mp4", "video/mp4");
    let png = encoded_image(ImageFormat::Png, 2, 1);
    fixture.write("small", "movie.png", &png);
    let mut headers = HeaderMap::new();
    headers.insert(RANGE, HeaderValue::from_static("bytes=0-7"));
    for method in [Method::GET, Method::HEAD] {
        let response =
            super::cached_remote_media_response(&fixture.state, &media, true, &method, &headers)
                .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[CONTENT_TYPE], "image/png");
        assert_eq!(response.headers()[CONTENT_LENGTH], "8");
        assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
        let bytes = axum::body::to_bytes(response.into_body(), 8).await.unwrap();
        assert_eq!(
            bytes.as_ref(),
            if method == Method::HEAD {
                &[]
            } else {
                &png[..8]
            }
        );
    }
}

fn authorized_cached_request(
    client: &reqwest::Client,
    method: Method,
    url: &str,
) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .header("host", "cached-media.invalid")
        .bearer_auth("fixture-bearer-read-statuses-v4-6-5")
}

async fn assert_explicit_thumbnail_storage_identity(
    owner: &PgPool,
    fixture: &CachedMediaFixture,
    client: &reqwest::Client,
    base: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let origin = Url::parse("https://cached-media.invalid/")?;
    let png = encoded_image(ImageFormat::Png, 2, 1);
    fixture.write("small", "normalized.png", &png);
    for remote in [None, Some("https://remote.fixture.invalid/real.png")] {
        sqlx::query("UPDATE media_attachments SET type=2, processing=2, file_file_name='normalized.mp4', file_content_type='video/mp4', thumbnail_file_name='real.png', thumbnail_content_type='image/png', thumbnail_storage_schema_version=1, thumbnail_remote_url=$2 WHERE id=$1")
            .bind(MEDIA_ID).bind(remote).execute(owner).await?;
        let thumbnail =
            "cache/media_attachments/thumbnails/000/012/001/original/real.png".to_owned();
        let path = fixture.root.join(&thumbnail);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, &png)?;
        let record = repository.remote_media_attachment(MEDIA_ID).await?.unwrap();
        let serialized = RestSerializer::new(
            &origin,
            "cached-media.invalid",
            "/system",
            Utc::now().naive_utc(),
        )
        .media_attachment(&media_projection(&record, None));
        let preview_path = Url::parse(serialized.preview_url.as_deref().unwrap())?
            .path()
            .trim_start_matches("/system/")
            .to_owned();
        // Explicit thumbnails and generated MediaFile small keep independent identities.
        for path in [
            preview_path,
            format!("{CACHE_DIRECTORY}/small/normalized.png"),
        ] {
            for (method, range) in [
                (Method::GET, false),
                (Method::HEAD, false),
                (Method::GET, true),
                (Method::HEAD, true),
            ] {
                let mut request = authorized_cached_request(
                    client,
                    method.clone(),
                    &format!("{base}/system/{path}"),
                );
                if range {
                    request = request.header("range", "bytes=0-7");
                }
                let response = request.send().await?;
                assert_eq!(
                    response.status(),
                    if range {
                        StatusCode::PARTIAL_CONTENT
                    } else {
                        StatusCode::OK
                    },
                    "{path} {method}"
                );
                assert_eq!(response.headers()[CONTENT_TYPE], "image/png");
                assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
                assert_eq!(
                    response.bytes().await?.as_ref(),
                    if method == Method::HEAD {
                        &[]
                    } else if range {
                        &png[..8]
                    } else {
                        &png
                    }
                );
            }
        }
        // A stale copy in the other namespace must not authorize the wrong URL.
        let stale = fixture
            .root
            .join("media_attachments/thumbnails/000/012/001/original/real.png");
        fs::create_dir_all(stale.parent().unwrap())?;
        fs::write(stale, &png)?;
        let response = authorized_cached_request(
            client,
            Method::GET,
            &format!("{base}/system/media_attachments/thumbnails/000/012/001/original/real.png"),
        )
        .send()
        .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    sqlx::query("UPDATE media_attachments SET processing=3 WHERE id=$1")
        .bind(MEDIA_ID)
        .execute(owner)
        .await?;
    let response = authorized_cached_request(
        client,
        Method::HEAD,
        &format!("{base}/system/cache/media_attachments/thumbnails/000/012/001/original/real.png"),
    )
    .header("range", "bytes=0-7")
    .send()
    .await?;
    assert_eq!(
        response.status(),
        StatusCode::PARTIAL_CONTENT,
        "separate thumbnail readiness is unchanged"
    );

    Ok(())
}

async fn assert_direct_remote_processing_states(
    owner: &PgPool,
    fixture: &CachedMediaFixture,
    client: &reqwest::Client,
    base: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for (kind, mime, name, small) in [
        (2, "video/mp4", "normalized.mp4", Some("normalized.png")),
        (4, "audio/mpeg", "normalized.mp3", None),
        (0, "image/jpeg", "normalized.jpeg", Some("normalized.jpeg")),
    ] {
        fixture.write("original", name, b"installed original");
        if let Some(small) = small {
            fixture.write("small", small, b"installed preview");
        }
        for processing in [Some(2), Some(0), Some(1), Some(3), None, Some(2)] {
            sqlx::query("UPDATE media_attachments SET type=$2, file_content_type=$3, file_file_name=$4, processing=$5 WHERE id=$1")
                .bind(MEDIA_ID).bind(kind).bind(mime).bind(name).bind(processing).execute(owner).await?;
            let ready = processing.is_none_or(|state| state == 2);
            for (style, name) in [("original", Some(name)), ("small", small)] {
                let Some(name) = name else {
                    continue;
                };
                let path = format!("{base}/system/{CACHE_DIRECTORY}/{style}/{name}");
                for (method, range) in [
                    (Method::GET, false),
                    (Method::HEAD, false),
                    (Method::GET, true),
                    (Method::HEAD, true),
                ] {
                    let mut request = authorized_cached_request(client, method.clone(), &path);
                    if range {
                        request = request.header("range", "bytes=0-7");
                    }
                    let response = request.send().await?;
                    assert_eq!(
                        response.status(),
                        if !ready {
                            StatusCode::NOT_FOUND
                        } else if range {
                            StatusCode::PARTIAL_CONTENT
                        } else {
                            StatusCode::OK
                        },
                        "{mime} {style} {processing:?} {method}"
                    );
                    assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
                    let bytes = response.bytes().await?;
                    if ready {
                        let expected = if style == "original" {
                            b"installed original".as_slice()
                        } else {
                            b"installed preview".as_slice()
                        };
                        assert_eq!(
                            bytes.as_ref(),
                            if method == Method::HEAD {
                                &[]
                            } else if range {
                                &expected[..8]
                            } else {
                                expected
                            }
                        );
                    } else if method != Method::HEAD {
                        assert_eq!(
                            serde_json::from_slice::<serde_json::Value>(&bytes)?,
                            serde_json::json!({"error": "Not Found"})
                        );
                    }
                }
            }
        }
    }
    // Do not turn the remote-file rule into a new attached-local readiness rule.
    let local_path = "media_attachments/files/000/012/001/original/normalized.mp4";
    let local_file = fixture.root.join(local_path);
    fs::create_dir_all(local_file.parent().unwrap())?;
    fs::write(local_file, b"historical attached local")?;
    for processing in [Some(0), Some(1), Some(3), None, Some(2)] {
        sqlx::query("UPDATE media_attachments SET remote_url='', type=2, file_content_type='video/mp4', file_file_name='normalized.mp4', processing=$2 WHERE id=$1")
            .bind(MEDIA_ID).bind(processing).execute(owner).await?;
        let response =
            authorized_cached_request(client, Method::GET, &format!("{base}/system/{local_path}"))
                .send()
                .await?;
        assert_eq!(response.status(), StatusCode::OK, "local {processing:?}");
        assert_eq!(
            response.bytes().await?.as_ref(),
            b"historical attached local"
        );
    }

    Ok(())
}

#[test]
fn explicit_thumbnail_metadata_uses_attachment_model_identity() {
    let mut media = media("normalized.mp4", "video/mp4");
    for remote_url in ["", "https://remote.invalid/source.mov"] {
        media.remote_url = remote_url.to_owned();
        for thumbnail_remote_url in [
            None,
            Some("https://remote.invalid/separate.webp".to_owned()),
        ] {
            media.thumbnail_remote_url = thumbnail_remote_url;
            let metadata = media_thumbnail_metadata_from_record(&media).unwrap();
            assert_eq!(metadata.remote, !remote_url.is_empty());
            let prefix = if remote_url.is_empty() { "" } else { "cache/" };
            assert_eq!(
                metadata.relative_path("original"),
                Some(format!(
                    "{prefix}media_attachments/thumbnails/000/012/001/original/separate.webp"
                ))
            );
        }
    }
}
