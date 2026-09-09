use std::fs;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use image::AnimationDecoder;
use image::codecs::gif::GifDecoder;
#[cfg(feature = "test-support")]
use rustodon::paperclip::PaperclipWriteFault;
use rustodon::paperclip::{
    PaperclipAttachment, PaperclipMetadata, PaperclipRoot, encode_url_path, open_paperclip_file,
    parse_paperclip_path, partitioned_id, prepare_account_media, prepare_media_attachment,
    write_prepared_media,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn metadata(
    attachment: PaperclipAttachment,
    id: i64,
    remote: bool,
    storage_schema_version: Option<i32>,
    file_name: &str,
    content_type: Option<&str>,
) -> PaperclipMetadata {
    PaperclipMetadata {
        attachment,
        id,
        remote,
        storage_schema_version,
        file_name: file_name.to_owned(),
        content_type: content_type.map(str::to_owned),
        variant: None,
    }
}

fn accepts(path: &str, metadata: &PaperclipMetadata) -> bool {
    parse_paperclip_path(path).is_some_and(|request| request.authorizes(metadata))
}

#[test]
fn partitions_match_paperclip_for_existing_id_shapes() {
    assert_eq!(partitioned_id(12_001).as_deref(), Some("000/012/001"));
    assert_eq!(
        partitioned_id(116_844_842_188_806_001).as_deref(),
        Some("116/844/842/188/806/001")
    );
    assert_eq!(partitioned_id(0), None);
    assert_eq!(partitioned_id(-1), None);
    assert_eq!(partitioned_id(1_000_000_000_000), None);
}

#[test]
fn fixture_paths_require_exact_database_metadata() {
    let cases = [
        (
            "accounts/avatars/116/844/606/259/201/001/original/0112603425bb49c1.png",
            metadata(
                PaperclipAttachment::AccountAvatar,
                116_844_606_259_201_001,
                false,
                Some(1),
                "0112603425bb49c1.png",
                Some("image/png"),
            ),
        ),
        (
            "cache/accounts/avatars/116/844/606/259/202/001/original/bob.png",
            metadata(
                PaperclipAttachment::AccountAvatar,
                116_844_606_259_202_001,
                true,
                Some(1),
                "bob.png",
                Some("image/png"),
            ),
        ),
        (
            "media_attachments/files/116/844/842/188/806/001/small/cd63911ad76f4d5d.jpg",
            metadata(
                PaperclipAttachment::MediaFile,
                116_844_842_188_806_001,
                false,
                None,
                "cd63911ad76f4d5d.jpg",
                Some("image/jpeg"),
            ),
        ),
        (
            "cache/media_attachments/files/116/845/105/643/526/106/original/cached.jpg",
            metadata(
                PaperclipAttachment::MediaFile,
                116_845_105_643_526_106,
                true,
                Some(1),
                "cached.jpg",
                Some("image/jpeg"),
            ),
        ),
        (
            "cache/custom_emojis/images/000/012/001/static/fixtureparty.png",
            metadata(
                PaperclipAttachment::CustomEmojiImage,
                12_001,
                true,
                Some(1),
                "fixtureparty.png",
                Some("image/png"),
            ),
        ),
        (
            "cache/preview_cards/images/000/012/002/original/preview.png",
            metadata(
                PaperclipAttachment::PreviewCardImage,
                12_002,
                true,
                Some(1),
                "preview.png",
                Some("image/png"),
            ),
        ),
    ];

    for (path, metadata) in cases {
        assert!(accepts(path, &metadata), "expected {path} to be authorized");
    }
}

#[test]
fn cache_prefix_and_attachment_versions_are_exact() {
    let mut remote = metadata(
        PaperclipAttachment::MediaFile,
        12_001,
        true,
        Some(1),
        "clip.jpg",
        Some("image/jpeg"),
    );
    assert!(accepts(
        "cache/media_attachments/files/000/012/001/original/clip.jpg",
        &remote
    ));
    assert!(!accepts(
        "media_attachments/files/000/012/001/original/clip.jpg",
        &remote
    ));

    remote.storage_schema_version = Some(0);
    assert!(accepts(
        "media_attachments/files/000/012/001/original/clip.jpg",
        &remote
    ));
    remote.storage_schema_version = None;
    assert!(accepts(
        "media_attachments/files/000/012/001/original/clip.jpg",
        &remote
    ));

    remote.remote = false;
    remote.storage_schema_version = Some(2);
    assert!(accepts(
        "media_attachments/files/000/012/001/original/clip.jpg",
        &remote
    ));
}

#[test]
fn styles_and_derivative_extensions_follow_attachment_metadata() {
    let gif_avatar = metadata(
        PaperclipAttachment::AccountAvatar,
        12_001,
        false,
        Some(1),
        "avatar.gif",
        Some("image/gif"),
    );
    assert!(accepts(
        "accounts/avatars/000/012/001/static/avatar.png",
        &gif_avatar
    ));
    assert!(!accepts(
        "accounts/avatars/000/012/001/static/avatar.gif",
        &gif_avatar
    ));

    let png_avatar = PaperclipMetadata {
        content_type: Some("image/png".to_owned()),
        ..gif_avatar.clone()
    };
    assert!(!accepts(
        "accounts/avatars/000/012/001/static/avatar.png",
        &png_avatar
    ));

    let emoji = metadata(
        PaperclipAttachment::CustomEmojiImage,
        12_001,
        false,
        None,
        "party.webp",
        Some("image/webp"),
    );
    assert!(accepts(
        "custom_emojis/images/000/012/001/static/party.png",
        &emoji
    ));

    let video = metadata(
        PaperclipAttachment::MediaFile,
        12_001,
        false,
        None,
        "movie.mp4",
        Some("video/mp4"),
    );
    assert!(accepts(
        "media_attachments/files/000/012/001/small/movie.png",
        &video
    ));

    let converted_image = PaperclipMetadata {
        file_name: "photo.avif".to_owned(),
        content_type: Some("image/avif".to_owned()),
        ..video.clone()
    };
    assert!(accepts(
        "media_attachments/files/000/012/001/small/photo.jpeg",
        &converted_image
    ));

    let audio = PaperclipMetadata {
        file_name: "sound.mp3".to_owned(),
        content_type: Some("audio/mpeg".to_owned()),
        ..video
    };
    assert!(!accepts(
        "media_attachments/files/000/012/001/small/sound.mp3",
        &audio
    ));
}

#[test]
fn thumbnails_previews_headers_provider_icons_and_site_uploads_are_bounded() {
    let thumbnail = metadata(
        PaperclipAttachment::MediaThumbnail,
        12_001,
        true,
        Some(1),
        "cover.jpg",
        Some("image/jpeg"),
    );
    assert!(accepts(
        "cache/media_attachments/thumbnails/000/012/001/original/cover.jpg",
        &thumbnail
    ));
    assert!(!accepts(
        "cache/media_attachments/thumbnails/000/012/001/small/cover.jpg",
        &thumbnail
    ));

    let header = metadata(
        PaperclipAttachment::AccountHeader,
        12_001,
        false,
        None,
        "header.png",
        Some("image/png"),
    );
    assert!(accepts(
        "accounts/headers/000/012/001/original/header.png",
        &header
    ));

    let provider = metadata(
        PaperclipAttachment::PreviewCardProviderIcon,
        12_001,
        false,
        None,
        "provider.webp",
        Some("image/webp"),
    );
    assert!(accepts(
        "preview_card_providers/icons/000/012/001/static/provider.png",
        &provider
    ));

    let mut upload = metadata(
        PaperclipAttachment::SiteUploadFile,
        12_001,
        false,
        None,
        "icon.webp",
        Some("image/webp"),
    );
    upload.variant = Some("app_icon".to_owned());
    assert!(accepts(
        "site_uploads/files/000/012/001/192/icon.png",
        &upload
    ));
    assert!(!accepts(
        "site_uploads/files/000/012/001/@2x/icon.png",
        &upload
    ));
    upload.variant = Some("thumbnail".to_owned());
    assert!(accepts(
        "site_uploads/files/000/012/001/@2x/icon.png",
        &upload
    ));
    upload.variant = Some("mascot".to_owned());
    assert!(accepts(
        "site_uploads/files/000/012/001/mascot/icon.webp",
        &upload
    ));
}

#[test]
fn malformed_and_unsafe_paths_are_rejected_before_lookup() {
    for path in [
        "../accounts/avatars/000/012/001/original/avatar.png",
        "%2e%2e/accounts/avatars/000/012/001/original/avatar.png",
        "accounts//avatars/000/012/001/original/avatar.png",
        "accounts/avatars/000/012/001/original%2Favatar.png",
        "accounts/avatars/000/012/001/original/avatar%00.png",
        "accounts/avatars/000/012/01/original/avatar.png",
        "accounts/avatars/000/012/001/unknown/avatar.png",
        "backups/dumps/000/012/001/original/archive.zip",
    ] {
        assert!(parse_paperclip_path(path).is_none(), "accepted {path}");
    }
}

#[test]
fn generated_url_paths_escape_filename_delimiters_by_component() {
    let path = "accounts/avatars/000/012/001/original/a #?%+.png";
    let encoded = encode_url_path(path);
    assert_eq!(
        encoded,
        "accounts/avatars/000/012/001/original/a%20%23%3F%25%2B.png"
    );
    assert!(parse_paperclip_path(&encoded).is_some());
}

#[test]
fn safe_open_rejects_intermediate_and_final_symlinks() {
    use std::os::unix::fs::symlink;

    let root = temp_root("paperclip-open");
    let relative = Path::new("accounts/avatars/000/012/001/original/avatar.png");
    fs::create_dir_all(root.join(relative.parent().unwrap())).unwrap();
    fs::write(root.join(relative), b"avatar").unwrap();

    let mut file = open_paperclip_file(&root, relative).expect("regular file should open");
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert_eq!(bytes, b"avatar");

    let outside = temp_root("paperclip-outside");
    fs::write(outside.join("secret"), b"secret").unwrap();
    fs::remove_file(root.join(relative)).unwrap();
    symlink(outside.join("secret"), root.join(relative)).unwrap();
    assert!(open_paperclip_file(&root, relative).is_err());

    fs::remove_file(root.join(relative)).unwrap();
    let avatars = root.join("accounts/avatars");
    fs::remove_dir_all(&avatars).unwrap();
    symlink(&outside, &avatars).unwrap();
    assert!(open_paperclip_file(&root, relative).is_err());

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[test]
fn safe_open_rejects_symlinks_in_the_root_path() {
    use std::os::unix::fs::symlink;

    let parent = temp_root("paperclip-root-link");
    let actual = parent.join("actual");
    fs::create_dir(&actual).unwrap();
    let link = parent.join("link");
    symlink(&actual, &link).unwrap();
    assert!(rustodon::paperclip::PaperclipRoot::open(&link).is_err());
    let _ = fs::remove_dir_all(parent);
}

#[test]
fn account_media_processing_uses_rails_profile_geometries() {
    let avatar_bytes = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/avatar.gif")
        .expect("Mastodon avatar fixture exists");
    let avatar = prepare_account_media(
        PaperclipAttachment::AccountAvatar,
        101,
        "avatar.gif",
        "image/gif",
        &avatar_bytes,
    )
    .expect("avatar fixture is valid");
    assert_eq!((avatar.width, avatar.height), (400, 400));
    assert!(extension_is(&avatar.file_name, "gif"));
    assert_eq!(
        avatar
            .static_file_name
            .as_deref()
            .map(|name| extension_is(name, "png")),
        Some(true)
    );
    assert!(avatar.static_bytes.is_some());
    let source_frames = GifDecoder::new(Cursor::new(avatar_bytes.clone()))
        .unwrap()
        .into_frames()
        .count();
    let output_frames = GifDecoder::new(Cursor::new(avatar.original_bytes.clone()))
        .unwrap()
        .into_frames()
        .count();
    assert_eq!(output_frames, source_frames.min(3000));

    let header_bytes = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.jpg")
        .expect("Mastodon header fixture exists");
    let header = prepare_account_media(
        PaperclipAttachment::AccountHeader,
        101,
        "header.jpg",
        "image/jpeg",
        &header_bytes,
    )
    .expect("header fixture is valid");
    assert_eq!((header.width, header.height), (600, 400));
    assert!(header.static_bytes.is_none());

    let png_bytes = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/emojo.png")
        .expect("Mastodon PNG fixture exists");
    let normalized = prepare_account_media(
        PaperclipAttachment::AccountAvatar,
        101,
        "photo.jpg",
        "image/png",
        &png_bytes,
    )
    .expect("mismatched extension is normalized");
    assert!(extension_is(&normalized.file_name, "png"));
    assert_eq!(normalized.file_name.len(), 20);
}

#[test]
fn image_media_processing_builds_original_and_small_metadata() {
    let bytes = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.jpg")
        .expect("Mastodon media fixture exists");
    let prepared = prepare_media_attachment(101, "attachment.jpg", "image/jpeg", &bytes)
        .expect("media fixture is valid");

    assert_eq!((prepared.width, prepared.height), (600, 400));
    assert_eq!((prepared.small_width, prepared.small_height), (588, 392));
    assert_ne!(prepared.original_bytes, bytes);
    assert!(extension_is(&prepared.file_name, "jpg"));
    assert_eq!(prepared.file_name.len(), 20);
    assert_eq!(prepared.small_file_name, prepared.file_name);
    assert_eq!(prepared.content_type, "image/jpeg");
    assert_eq!(
        prepared.file_size,
        i32::try_from(prepared.original_bytes.len()).unwrap()
    );
    assert_eq!(prepared.blurhash.as_ref().map(String::len), Some(36));
    assert_eq!(
        prepared.file_meta["original"]["size"],
        serde_json::Value::String("600x400".to_owned())
    );
    assert_eq!(
        prepared.file_meta["small"]["size"],
        serde_json::Value::String("588x392".to_owned())
    );
}

#[test]
fn prepared_remote_media_writes_under_the_cache_prefix() {
    let bytes = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.jpg")
        .expect("Mastodon media fixture exists");
    let prepared = prepare_media_attachment(101, "attachment.jpg", "image/jpeg", &bytes)
        .expect("media fixture is valid");
    let root_path = temp_root("remote-media");
    let root = PaperclipRoot::open(&root_path).expect("root is safe");
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: 101,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };

    root.write_file(
        Path::new(&metadata.relative_path("original").unwrap()),
        b"partial-original",
    )
    .expect("partial original is written");
    root.write_file(
        Path::new(&metadata.relative_path("small").unwrap()),
        b"partial-small",
    )
    .expect("partial derivative is written");
    let paths = write_prepared_media(&root, &metadata, &prepared).expect("media is written");
    assert!(paths.iter().all(|path| path.starts_with("cache/")));
    for (path, expected) in [
        (&paths[0], &prepared.original_bytes),
        (&paths[1], &prepared.small_bytes),
    ] {
        let mut file = root.open_file(Path::new(path)).expect("media is readable");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
        assert_eq!(&bytes, expected);
    }
    let _ = fs::remove_dir_all(root_path);
}

#[test]
fn prepared_media_write_removes_original_when_derivative_storage_fails() {
    let bytes = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.jpg")
        .expect("Mastodon media fixture exists");
    let prepared = prepare_media_attachment(102, "attachment.jpg", "image/jpeg", &bytes)
        .expect("media fixture is valid");
    let root_path = temp_root("media-write-failure");
    let root = PaperclipRoot::open(&root_path).expect("root is safe");
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: 102,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let small = Path::new(&metadata.relative_path("small").unwrap()).to_owned();
    let small_directory = small.parent().expect("small path has a directory");
    fs::create_dir_all(root_path.join(small_directory.parent().unwrap()))
        .expect("small parent is created");
    fs::write(root_path.join(small_directory), b"not a directory")
        .expect("derivative blocker is created");

    assert!(write_prepared_media(&root, &metadata, &prepared).is_err());
    assert!(
        root.open_file(Path::new(&metadata.relative_path("original").unwrap()))
            .is_err(),
        "a failed derivative write must not leave the original file behind"
    );
    let _ = fs::remove_dir_all(root_path);
}

#[cfg(feature = "test-support")]
#[test]
fn prepared_media_write_recovers_after_injected_storage_full() {
    let bytes = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.jpg")
        .expect("Mastodon media fixture exists");
    let prepared = prepare_media_attachment(103, "attachment.jpg", "image/jpeg", &bytes)
        .expect("media fixture is valid");
    let root_path = temp_root("media-write-storage-full");
    let root = PaperclipRoot::open(&root_path)
        .expect("root is safe")
        .with_write_fault(PaperclipWriteFault::storage_full_after(1));
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: 103,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let original_path = Path::new(&metadata.relative_path("original").unwrap()).to_owned();
    let small_path = Path::new(&metadata.relative_path("small").unwrap()).to_owned();

    let error = write_prepared_media(&root, &metadata, &prepared)
        .expect_err("the injected storage-full error must reach the caller");
    assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
    assert!(root.open_file(&original_path).is_err());
    assert!(root.open_file(&small_path).is_err());

    let paths = write_prepared_media(&root, &metadata, &prepared)
        .expect("the consumed fault must not poison a retry");
    assert_eq!(
        paths,
        vec![
            metadata.relative_path("original").unwrap(),
            metadata.relative_path("small").unwrap(),
        ]
    );

    let _ = fs::remove_dir_all(root_path);
}

#[test]
fn paperclip_root_can_write_and_remove_clean_relative_files() {
    let root = temp_root("paperclip-write");
    let relative = Path::new("accounts/avatars/000/000/101/original/avatar.gif");
    let paperclip = PaperclipRoot::open(&root).expect("root is safe");
    paperclip
        .write_file(relative, b"avatar")
        .expect("file is written");
    assert_eq!(
        fs::metadata(root.join(relative))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    paperclip
        .write_file(relative, b"replacement")
        .expect("existing files are not truncated");
    let mut file = paperclip
        .open_file(relative)
        .expect("written file is readable");
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert_eq!(bytes, b"avatar");
    paperclip
        .remove_file(relative)
        .expect("written file is removed");
    assert!(paperclip.open_file(relative).is_err());
    let _ = fs::remove_dir_all(root);
}

fn temp_root(label: &str) -> std::path::PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "rustodon-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    root
}

fn extension_is(file_name: &str, extension: &str) -> bool {
    Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}
