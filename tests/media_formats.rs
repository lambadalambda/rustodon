use rustodon::media::{ALL_MEDIA_MIME_TYPES, MediaKind, media_format};

#[test]
fn advertised_media_formats_share_one_processing_contract() {
    assert_eq!(ALL_MEDIA_MIME_TYPES.len(), 28);
    for content_type in ALL_MEDIA_MIME_TYPES {
        assert!(
            media_format(content_type).is_some(),
            "advertised MIME has no processing contract: {content_type}"
        );
    }
    assert!(media_format("application/octet-stream").is_none());
}

#[test]
fn media_contract_normalizes_modern_stills_video_and_audio() {
    let heic = media_format("image/heic").unwrap();
    assert_eq!(heic.kind, MediaKind::Image);
    assert!(heic.external_processing);
    assert_eq!(heic.output_content_type, "image/jpeg");
    assert_eq!(heic.preview_content_type, Some("image/jpeg"));
    assert_eq!(heic.input_size_limit, 16 * 1024 * 1024);

    let video = media_format("video/webm").unwrap();
    assert_eq!(video.kind, MediaKind::Video);
    assert!(video.external_processing);
    assert_eq!(video.output_content_type, "video/mp4");
    assert_eq!(video.preview_content_type, Some("image/png"));
    assert_eq!(video.input_size_limit, 99 * 1024 * 1024);

    let audio = media_format("audio/ogg").unwrap();
    assert_eq!(audio.kind, MediaKind::Audio);
    assert!(audio.external_processing);
    assert_eq!(audio.output_content_type, "audio/mpeg");
    assert_eq!(audio.preview_content_type, None);
    assert_eq!(audio.input_size_limit, 99 * 1024 * 1024);
}

#[test]
fn legacy_images_keep_the_existing_outputs() {
    let jpeg = media_format("image/jpeg").unwrap();
    assert_eq!(jpeg.kind, MediaKind::Image);
    assert!(!jpeg.external_processing);
    assert_eq!(jpeg.output_content_type, "image/jpeg");
    assert_eq!(jpeg.preview_content_type, Some("image/jpeg"));

    let gif = media_format("image/gif").unwrap();
    assert_eq!(gif.kind, MediaKind::Gif);
    assert!(!gif.external_processing);
    assert_eq!(gif.output_content_type, "image/gif");
    assert_eq!(gif.preview_content_type, Some("image/png"));
}

#[test]
fn remote_media_policy_normalizes_and_requires_mime_agreement() {
    use rustodon::media::RemoteMediaPolicy;
    for mime in ALL_MEDIA_MIME_TYPES {
        let policy =
            RemoteMediaPolicy::new(Some(&format!(" {} ; charset=binary", mime.to_uppercase())))
                .unwrap();
        assert_eq!(policy.response_format(mime), media_format(mime));
        assert_eq!(
            RemoteMediaPolicy::new(None).unwrap().response_format(mime),
            media_format(mime)
        );
    }
    let video = RemoteMediaPolicy::new(Some("video/mp4")).unwrap();
    for mismatch in [
        "video/webm",
        "audio/mp4",
        "image/png",
        "application/octet-stream",
        "",
    ] {
        assert!(video.response_format(mismatch).is_none());
    }
    for unsupported in [
        "",
        "application/octet-stream",
        "image/*",
        "video/mp4, image/png",
    ] {
        assert!(RemoteMediaPolicy::new(Some(unsupported)).is_none());
        assert!(
            RemoteMediaPolicy::new(None)
                .unwrap()
                .response_format(unsupported)
                .is_none()
        );
    }
}
