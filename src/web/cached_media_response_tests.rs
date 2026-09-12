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
