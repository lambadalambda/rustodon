use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
#[cfg(feature = "test-support")]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::media::{MediaFormat, MediaKind, media_format};
use image::codecs::gif::{GifDecoder, GifEncoder};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{
    AnimationDecoder, Delay, Frame, GenericImageView, ImageDecoder, ImageFormat, ImageReader,
    Limits,
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use rustix::fd::OwnedFd;
use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, ResolveFlags, fchmod, fstat, fsync, mkdirat, open, openat2,
    unlinkat,
};
use rustix::io::Errno;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const APP_ICON_STYLES: &[&str] = &[
    "36", "48", "57", "60", "72", "76", "96", "114", "120", "144", "152", "167", "180", "192",
    "256", "384", "512", "1024",
];
const FAVICON_STYLES: &[&str] = &["16", "32", "48"];
const THUMBNAIL_STYLES: &[&str] = &["@1x", "@2x"];
const ACCOUNT_MEDIA_LIMIT: usize = 8 * 1024 * 1024;
const MAX_MATRIX_LIMIT: u64 = 33_177_600;
const GIF_MATRIX_LIMIT: u64 = 921_600;
const CUSTOM_EMOJI_GIF_MAX_FRAMES: usize = 256;
const CUSTOM_EMOJI_GIF_MAX_PIXELS: u64 = GIF_MATRIX_LIMIT * 16;
const MEDIA_MATRIX_LIMIT: u64 = 8_294_400;
const MEDIA_PROCESS_TIMEOUT: Duration = Duration::from_mins(1);
const MEDIA_MAX_DURATION_SECONDS: f64 = 3_600.0;
const MEDIA_MAX_FRAME_RATE: f64 = 120.0;
const MEDIA_MAX_FRAMES: u64 = 36_000;
const MEDIA_PROBE_OUTPUT_LIMIT: usize = 64 * 1024;
const MEDIA_STDERR_LIMIT: usize = 64 * 1024;
const MEDIA_STOP_DURATION: &str = "3600.1";
const MEDIA_STOP_FRAMES: &str = "36001";
const IMAGE_MAX_ALLOC: u64 = 256 * 1024 * 1024;
const URL_PATH_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'*')
    .remove(b',')
    .remove(b'-')
    .remove(b'.')
    .remove(b':')
    .remove(b';')
    .remove(b'=')
    .remove(b'@')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaperclipAttachment {
    AccountAvatar,
    AccountHeader,
    MediaFile,
    MediaThumbnail,
    CustomEmojiImage,
    PreviewCardImage,
    PreviewCardProviderIcon,
    SiteUploadFile,
}

impl PaperclipAttachment {
    const fn path(self) -> &'static str {
        match self {
            Self::AccountAvatar => "accounts/avatars",
            Self::AccountHeader => "accounts/headers",
            Self::MediaFile => "media_attachments/files",
            Self::MediaThumbnail => "media_attachments/thumbnails",
            Self::CustomEmojiImage => "custom_emojis/images",
            Self::PreviewCardImage => "preview_cards/images",
            Self::PreviewCardProviderIcon => "preview_card_providers/icons",
            Self::SiteUploadFile => "site_uploads/files",
        }
    }

    const fn permits_cache(self) -> bool {
        matches!(
            self,
            Self::AccountAvatar
                | Self::AccountHeader
                | Self::MediaFile
                | Self::MediaThumbnail
                | Self::CustomEmojiImage
                | Self::PreviewCardImage
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaperclipMetadata {
    pub attachment: PaperclipAttachment,
    pub id: i64,
    pub remote: bool,
    pub storage_schema_version: Option<i32>,
    pub file_name: String,
    pub content_type: Option<String>,
    /// Attachment-specific discriminator, currently the `site_uploads.var` value.
    pub variant: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PreparedAccountMedia {
    pub file_name: String,
    pub content_type: String,
    pub file_size: i32,
    pub original_bytes: Vec<u8>,
    pub static_file_name: Option<String>,
    pub static_bytes: Option<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct PreparedMediaAttachment {
    pub media_kind: MediaKind,
    pub file_name: String,
    pub content_type: String,
    pub file_size: i32,
    pub original_bytes: Vec<u8>,
    pub small_file_name: Option<String>,
    pub small_content_type: Option<String>,
    pub small_bytes: Option<Vec<u8>>,
    pub file_meta: Value,
    pub blurhash: Option<String>,
    pub width: u32,
    pub height: u32,
    pub small_width: u32,
    pub small_height: u32,
}

#[derive(Clone, Debug)]
pub struct PreparedCustomEmoji {
    pub file_name: String,
    pub content_type: String,
    pub file_size: i32,
    pub original_bytes: Vec<u8>,
    pub static_bytes: Vec<u8>,
}

#[cfg(feature = "test-support")]
#[derive(Debug)]
pub struct PaperclipWriteFault {
    successful_writes_before_failure: AtomicUsize,
    injected: AtomicBool,
}

#[cfg(feature = "test-support")]
impl PaperclipWriteFault {
    #[must_use]
    pub fn storage_full_after(successful_writes: usize) -> Self {
        Self {
            successful_writes_before_failure: AtomicUsize::new(successful_writes),
            injected: AtomicBool::new(false),
        }
    }

    fn should_fail(&self) -> bool {
        loop {
            if self.injected.load(Ordering::Acquire) {
                return false;
            }
            let remaining = self
                .successful_writes_before_failure
                .load(Ordering::Relaxed);
            if remaining == 0 {
                return self
                    .injected
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok();
            }
            if self
                .successful_writes_before_failure
                .compare_exchange_weak(
                    remaining,
                    remaining - 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return false;
            }
        }
    }
}

#[cfg(feature = "test-support")]
#[derive(Debug)]
pub struct PaperclipCommitFault {
    fail_before_commit: AtomicBool,
    fail_after_commit: AtomicBool,
}

#[cfg(feature = "test-support")]
#[derive(Debug)]
pub struct PaperclipRemoveFault {
    remaining: AtomicUsize,
}

#[cfg(feature = "test-support")]
#[derive(Debug)]
pub struct PaperclipDirectorySyncFault {
    fail: AtomicBool,
}

#[cfg(feature = "test-support")]
impl PaperclipDirectorySyncFault {
    #[must_use]
    pub fn fail_once() -> Self {
        Self {
            fail: AtomicBool::new(true),
        }
    }

    fn should_fail(&self) -> bool {
        self.fail.swap(false, Ordering::AcqRel)
    }
}

#[cfg(feature = "test-support")]
impl PaperclipRemoveFault {
    #[must_use]
    pub fn fail_once() -> Self {
        Self::fail_times(1)
    }

    #[must_use]
    pub fn fail_times(failures: usize) -> Self {
        Self {
            remaining: AtomicUsize::new(failures),
        }
    }

    fn should_fail(&self) -> bool {
        self.remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }
}

#[cfg(feature = "test-support")]
impl PaperclipCommitFault {
    #[must_use]
    pub fn before_and_after() -> Self {
        Self {
            fail_before_commit: AtomicBool::new(true),
            fail_after_commit: AtomicBool::new(true),
        }
    }

    fn take_before_commit(&self) -> bool {
        self.fail_before_commit.swap(false, Ordering::AcqRel)
    }

    fn take_after_commit(&self) -> bool {
        self.fail_after_commit.swap(false, Ordering::AcqRel)
    }
}

type ProcessedGif = (Vec<u8>, Option<Vec<u8>>, u32, u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountMediaError {
    UnsupportedAttachment,
    UnsupportedContentType,
    TooLarge,
    InvalidImage,
    SizeOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaAttachmentError {
    UnsupportedContentType,
    TooLarge,
    InvalidImage,
    InvalidMedia,
    ProcessingUnavailable,
    ProcessingTimedOut,
    SizeOverflow,
}

impl std::fmt::Display for MediaAttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedContentType => "unsupported media content type",
            Self::TooLarge => "media is too large",
            Self::InvalidImage => "media image is invalid",
            Self::InvalidMedia => "media contents do not match a supported format",
            Self::ProcessingUnavailable => "media processor is unavailable",
            Self::ProcessingTimedOut => "media processing timed out",
            Self::SizeOverflow => "processed media is too large",
        })
    }
}

impl std::error::Error for MediaAttachmentError {}

impl std::fmt::Display for AccountMediaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedAttachment => "unsupported account media attachment",
            Self::UnsupportedContentType => "unsupported account image content type",
            Self::TooLarge => "account image is too large",
            Self::InvalidImage => "account image is invalid",
            Self::SizeOverflow => "account image size is too large",
        })
    }
}

impl std::error::Error for AccountMediaError {}

/// Validates and processes a local avatar or header using Mastodon's profile styles.
///
/// The returned bytes are ready to be stored beneath the account's Paperclip paths. The source
/// image is processed using Mastodon's avatar/header styles; animated GIFs also receive the static
/// PNG derivative used by REST serializers.
///
/// # Errors
///
/// Returns an error when the attachment, content type, image dimensions, encoded size, or image
/// bytes are not accepted by Mastodon's profile media contract.
pub fn prepare_account_media(
    attachment: PaperclipAttachment,
    account_id: i64,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<PreparedAccountMedia, AccountMediaError> {
    if !matches!(
        attachment,
        PaperclipAttachment::AccountAvatar | PaperclipAttachment::AccountHeader
    ) {
        return Err(AccountMediaError::UnsupportedAttachment);
    }
    if !matches!(
        content_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    ) {
        return Err(AccountMediaError::UnsupportedContentType);
    }
    if bytes.len() >= ACCOUNT_MEDIA_LIMIT {
        return Err(AccountMediaError::TooLarge);
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| AccountMediaError::InvalidImage)?;
    let format = reader.format().ok_or(AccountMediaError::InvalidImage)?;
    if image_format_for_content_type(content_type) != Some(format) {
        return Err(AccountMediaError::InvalidImage);
    }
    let (input_width, input_height) = reader
        .into_dimensions()
        .map_err(|_| AccountMediaError::InvalidImage)?;
    validate_image_matrix(content_type, input_width, input_height)?;
    let file_name = media_file_name(file_name, content_type, account_id, bytes);
    let (original_bytes, static_bytes, width, height) = if format == ImageFormat::Gif {
        process_gif(bytes, attachment, input_width, input_height)?
    } else {
        let mut reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|_| AccountMediaError::InvalidImage)?;
        reader.limits(image_limits());
        let image = reader
            .decode()
            .map_err(|_| AccountMediaError::InvalidImage)?;
        let image = transform_profile_image(image, attachment, input_width, input_height);
        let (width, height) = image.dimensions();
        let output_format = image_format_for_content_type(content_type)
            .ok_or(AccountMediaError::UnsupportedContentType)?;
        (encode_image(&image, output_format)?, None, width, height)
    };
    let static_file_name = static_bytes
        .as_ref()
        .and_then(|_| derivative_file_name(&file_name, "png"));
    let file_size =
        i32::try_from(original_bytes.len()).map_err(|_| AccountMediaError::SizeOverflow)?;
    Ok(PreparedAccountMedia {
        file_name,
        content_type: content_type.to_owned(),
        file_size,
        original_bytes,
        static_file_name,
        static_bytes,
        width,
        height,
    })
}

/// Validates and prepares a local image media attachment using Mastodon's image styles.
///
/// The original upload is retained byte-for-byte. The small style is encoded using the input
/// image format, except animated GIFs which use a PNG preview as in Paperclip.
///
/// # Errors
///
/// Returns an error when the image type, encoded size, dimensions, or image bytes are rejected.
pub fn prepare_media_attachment(
    account_id: i64,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<PreparedMediaAttachment, MediaAttachmentError> {
    let contract = media_format(content_type)
        .filter(|format| {
            !format.external_processing && matches!(format.kind, MediaKind::Image | MediaKind::Gif)
        })
        .ok_or(MediaAttachmentError::UnsupportedContentType)?;
    if bytes.len() >= contract.input_size_limit {
        return Err(MediaAttachmentError::TooLarge);
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| MediaAttachmentError::InvalidImage)?;
    let format = reader.format().ok_or(MediaAttachmentError::InvalidImage)?;
    if image_format_for_content_type(content_type) != Some(format) {
        return Err(MediaAttachmentError::InvalidImage);
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| MediaAttachmentError::InvalidImage)?;
    validate_image_matrix(content_type, width, height)
        .map_err(|_| MediaAttachmentError::InvalidImage)?;

    let image = if format == ImageFormat::Gif {
        let mut decoder =
            GifDecoder::new(Cursor::new(bytes)).map_err(|_| MediaAttachmentError::InvalidImage)?;
        decoder
            .set_limits(image_limits())
            .map_err(|_| MediaAttachmentError::InvalidImage)?;
        let frame = decoder
            .into_frames()
            .next()
            .ok_or(MediaAttachmentError::InvalidImage)?
            .map_err(|_| MediaAttachmentError::InvalidImage)?;
        image::DynamicImage::ImageRgba8(frame.into_buffer())
    } else {
        let mut reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|_| MediaAttachmentError::InvalidImage)?;
        reader.limits(image_limits());
        reader
            .decode()
            .map_err(|_| MediaAttachmentError::InvalidImage)?
    };
    let original_image = resize_media_image_to_limit(image, width, height, MEDIA_MATRIX_LIMIT);
    let (original_width, original_height) = original_image.dimensions();
    let original_bytes = if format == ImageFormat::Gif {
        bytes.to_vec()
    } else {
        encode_image(&original_image, format).map_err(|_| MediaAttachmentError::InvalidImage)?
    };
    let small = resize_media_image(original_image, original_width, original_height);
    let (small_width, small_height) = small.dimensions();
    let small_format = if format == ImageFormat::Gif {
        ImageFormat::Png
    } else {
        format
    };
    let small_bytes =
        encode_image(&small, small_format).map_err(|_| MediaAttachmentError::InvalidImage)?;
    let blurhash = media_blurhash(&small);
    let file_name = media_file_name(file_name, content_type, account_id, bytes);
    let small_file_name = if format == ImageFormat::Gif {
        derivative_file_name(&file_name, "png").ok_or(MediaAttachmentError::InvalidImage)?
    } else {
        file_name.clone()
    };
    let file_size =
        i32::try_from(original_bytes.len()).map_err(|_| MediaAttachmentError::SizeOverflow)?;
    Ok(PreparedMediaAttachment {
        media_kind: if format == ImageFormat::Gif {
            MediaKind::Gif
        } else {
            MediaKind::Image
        },
        file_name,
        content_type: content_type.to_owned(),
        file_size,
        original_bytes,
        small_file_name: Some(small_file_name),
        small_content_type: Some(if format == ImageFormat::Gif {
            "image/png".to_owned()
        } else {
            content_type.to_owned()
        }),
        small_bytes: Some(small_bytes),
        file_meta: json!({
            "original": image_geometry(original_width, original_height),
            "small": image_geometry(small_width, small_height),
        }),
        blurhash,
        width: original_width,
        height: original_height,
        small_width,
        small_height,
    })
}

#[derive(Clone, Debug)]
struct ProcessorConfig {
    ffprobe: PathBuf,
    ffmpeg: PathBuf,
    timeout: Duration,
}

impl Default for ProcessorConfig {
    fn default() -> Self {
        Self {
            ffprobe: PathBuf::from("ffprobe"),
            ffmpeg: PathBuf::from("ffmpeg"),
            timeout: MEDIA_PROCESS_TIMEOUT,
        }
    }
}

/// Injectable rich-media processor executables and deadline for deterministic tests.
#[cfg(feature = "test-support")]
#[derive(Clone, Debug)]
pub struct MediaProcessorConfig(ProcessorConfig);

#[cfg(feature = "test-support")]
impl MediaProcessorConfig {
    #[must_use]
    pub fn new(ffprobe: impl Into<PathBuf>, ffmpeg: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self(ProcessorConfig {
            ffprobe: ffprobe.into(),
            ffmpeg: ffmpeg.into(),
            timeout,
        })
    }
}

#[derive(Debug)]
struct ChildOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildError {
    Unavailable,
    TimedOut,
    Overflow,
    Failed,
}

/// Verifies that the installed processor can execute every advertised normalization path.
///
/// The check uses tiny pinned local fixtures and the same bounded, pipe-only commands as uploads.
///
/// # Errors
///
/// Returns a processor or validation error when a required executable, demuxer, decoder, encoder,
/// or muxer is unavailable.
pub async fn validate_media_processor_capabilities() -> Result<(), MediaAttachmentError> {
    validate_media_processor_capabilities_inner(&ProcessorConfig::default()).await
}

/// Test-support capability check with injectable processor executables and deadline.
///
/// # Errors
///
/// Returns the same errors as [`validate_media_processor_capabilities`].
#[cfg(feature = "test-support")]
pub async fn validate_media_processor_capabilities_with_config(
    config: &MediaProcessorConfig,
) -> Result<(), MediaAttachmentError> {
    validate_media_processor_capabilities_inner(&config.0).await
}

async fn validate_media_processor_capabilities_inner(
    config: &ProcessorConfig,
) -> Result<(), MediaAttachmentError> {
    let deadline = tokio::time::Instant::now() + config.timeout;
    for (name, content_type, bytes) in [
        (
            "capability.avif",
            "image/avif",
            include_bytes!("../tests/fixtures/media/600x400.avif").as_slice(),
        ),
        (
            "capability.heic",
            "image/heic",
            include_bytes!("../tests/fixtures/media/600x400.heic").as_slice(),
        ),
        (
            "capability.webm",
            "video/webm",
            include_bytes!("../tests/fixtures/media/attachment.webm").as_slice(),
        ),
        (
            "capability.ogg",
            "audio/ogg",
            include_bytes!("../tests/fixtures/media/boop.ogg").as_slice(),
        ),
        (
            "capability.mp4",
            "video/mp4",
            include_bytes!("../tests/fixtures/media/capability.mp4").as_slice(),
        ),
        (
            "capability.mov",
            "video/quicktime",
            include_bytes!("../tests/fixtures/media/capability.mov").as_slice(),
        ),
        (
            "capability-audio.webm",
            "audio/webm",
            include_bytes!("../tests/fixtures/media/capability-audio.webm").as_slice(),
        ),
        (
            "capability-video.ogg",
            "video/ogg",
            include_bytes!("../tests/fixtures/media/capability-video.ogg").as_slice(),
        ),
        (
            "capability.wav",
            "audio/wav",
            include_bytes!("../tests/fixtures/media/capability.wav").as_slice(),
        ),
        (
            "capability.mp3",
            "audio/mpeg",
            include_bytes!("../tests/fixtures/media/capability.mp3").as_slice(),
        ),
        (
            "capability.flac",
            "audio/flac",
            include_bytes!("../tests/fixtures/media/capability.flac").as_slice(),
        ),
        (
            "capability.aac",
            "audio/aac",
            include_bytes!("../tests/fixtures/media/capability.aac").as_slice(),
        ),
        (
            "capability.m4a",
            "audio/m4a",
            include_bytes!("../tests/fixtures/media/capability.m4a").as_slice(),
        ),
        (
            "capability.3gp",
            "audio/3gpp",
            include_bytes!("../tests/fixtures/media/capability.3gp").as_slice(),
        ),
        (
            "capability.asf",
            "video/x-ms-asf",
            include_bytes!("../tests/fixtures/media/capability.asf").as_slice(),
        ),
    ] {
        let mut bounded = config.clone();
        bounded.timeout = remaining_timeout(deadline)?;
        prepare_rich_media_attachment_inner(1, name, content_type, bytes, &bounded).await?;
    }
    validate_aac_encoder(config, deadline).await
}

async fn validate_aac_encoder(
    config: &ProcessorConfig,
    deadline: tokio::time::Instant,
) -> Result<(), MediaAttachmentError> {
    let arguments = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=8000:cl=mono",
        "-t",
        "0.01",
        "-c:a",
        "aac",
        "-threads",
        "2",
        "-f",
        "adts",
        "pipe:1",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let output = run_child(
        config.ffmpeg.clone(),
        arguments,
        Vec::new(),
        MEDIA_PROBE_OUTPUT_LIMIT,
        MEDIA_STDERR_LIMIT,
        remaining_timeout(deadline)?,
    )
    .await
    .map_err(media_child_error)?;
    if output.stdout.is_empty() {
        return Err(MediaAttachmentError::ProcessingUnavailable);
    }
    Ok(())
}

/// Validates and prepares every media format advertised to REST clients.
///
/// Legacy browser images retain the in-process path. Modern stills, video, and audio are
/// normalized through bounded local `ffprobe`/`ffmpeg` child processes. Input and output use only
/// anonymous pipes, and the declared MIME type selects the input demuxer.
///
/// # Errors
///
/// Returns a validation or processor error when the declared type, bytes, metadata, limits, child
/// process, or generated outputs are invalid.
pub async fn prepare_rich_media_attachment(
    account_id: i64,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<PreparedMediaAttachment, MediaAttachmentError> {
    prepare_rich_media_attachment_inner(
        account_id,
        file_name,
        content_type,
        bytes,
        &ProcessorConfig::default(),
    )
    .await
}

/// Test-support entry point with injectable processor executables and deadline.
///
/// # Errors
///
/// Returns the same errors as [`prepare_rich_media_attachment`].
#[cfg(feature = "test-support")]
pub async fn prepare_rich_media_attachment_with_config(
    account_id: i64,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
    config: &MediaProcessorConfig,
) -> Result<PreparedMediaAttachment, MediaAttachmentError> {
    prepare_rich_media_attachment_inner(account_id, file_name, content_type, bytes, &config.0).await
}

async fn prepare_rich_media_attachment_inner(
    account_id: i64,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
    config: &ProcessorConfig,
) -> Result<PreparedMediaAttachment, MediaAttachmentError> {
    let format = media_format(content_type).ok_or(MediaAttachmentError::UnsupportedContentType)?;
    if bytes.is_empty() || bytes.len() >= format.input_size_limit {
        return Err(MediaAttachmentError::TooLarge);
    }
    if !format.external_processing {
        return prepare_media_attachment(account_id, file_name, content_type, bytes);
    }

    let deadline = tokio::time::Instant::now() + config.timeout;
    let demuxer = demuxer_for_content_type(content_type)
        .ok_or(MediaAttachmentError::UnsupportedContentType)?;
    if !media_container_matches(content_type, bytes) {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    let probe = probe_media(config, bytes, demuxer, format.kind, false, deadline).await?;
    inspect_probe(&probe, format.kind, content_type)?;

    match format.kind {
        MediaKind::Image => {
            prepare_modern_still(
                account_id, file_name, bytes, demuxer, format, config, deadline,
            )
            .await
        }
        MediaKind::Video => {
            prepare_video_attachment(
                account_id, file_name, bytes, demuxer, format, config, deadline,
            )
            .await
        }
        MediaKind::Audio => {
            prepare_audio_attachment(
                account_id, file_name, bytes, demuxer, format, config, deadline,
            )
            .await
        }
        MediaKind::Gif => Err(MediaAttachmentError::InvalidMedia),
    }
}

async fn prepare_modern_still(
    account_id: i64,
    file_name: &str,
    source_bytes: &[u8],
    demuxer: &'static str,
    format: MediaFormat,
    config: &ProcessorConfig,
    deadline: tokio::time::Instant,
) -> Result<PreparedMediaAttachment, MediaAttachmentError> {
    let output = run_ffmpeg(
        config,
        deadline,
        source_bytes,
        demuxer,
        MAX_MATRIX_LIMIT,
        &[
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-an",
            "-sn",
            "-dn",
            "-c:v",
            "mjpeg",
            "-q:v",
            "2",
            "-f",
            "image2pipe",
            "pipe:1",
        ],
        format.input_size_limit - 1,
    )
    .await?;
    prepare_media_attachment(
        account_id,
        file_name,
        format.output_content_type,
        &output.stdout,
    )
    .map_err(|_| MediaAttachmentError::InvalidMedia)
}

#[allow(clippy::too_many_lines)]
async fn prepare_video_attachment(
    account_id: i64,
    file_name: &str,
    source_bytes: &[u8],
    demuxer: &'static str,
    format: MediaFormat,
    config: &ProcessorConfig,
    deadline: tokio::time::Instant,
) -> Result<PreparedMediaAttachment, MediaAttachmentError> {
    let output = run_ffmpeg(
        config,
        deadline,
        source_bytes,
        demuxer,
        MEDIA_MATRIX_LIMIT,
        &[
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-t",
            MEDIA_STOP_DURATION,
            "-frames:v",
            MEDIA_STOP_FRAMES,
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-movflags",
            "+frag_keyframe+empty_moov+default_base_moof",
            "-f",
            "mp4",
            "pipe:1",
        ],
        format.input_size_limit - 1,
    )
    .await?;
    if !media_container_matches(format.output_content_type, &output.stdout) {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    let output_probe = probe_media(
        config,
        &output.stdout,
        "mov",
        MediaKind::Video,
        true,
        deadline,
    )
    .await?;
    let inspected = inspect_normalized_video(&output_probe)?;

    let preview = run_ffmpeg(
        config,
        deadline,
        &output.stdout,
        "mov",
        MEDIA_MATRIX_LIMIT,
        &[
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-an",
            "-sn",
            "-dn",
            "-vf",
            "scale='min(640,iw)':'min(640,ih)':force_original_aspect_ratio=decrease",
            "-c:v",
            "png",
            "-f",
            "image2pipe",
            "pipe:1",
        ],
        crate::media::IMAGE_SIZE_LIMIT - 1,
    )
    .await?;
    let preview_content_type = format
        .preview_content_type
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    let prepared_preview = prepare_media_attachment(
        account_id,
        "preview.png",
        preview_content_type,
        &preview.stdout,
    )
    .map_err(|_| MediaAttachmentError::InvalidMedia)?;
    let small_bytes = prepared_preview
        .small_bytes
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    let file_name = media_file_name(
        file_name,
        format.output_content_type,
        account_id,
        source_bytes,
    );
    let small_file_name =
        derivative_file_name(&file_name, "png").ok_or(MediaAttachmentError::InvalidMedia)?;
    let file_size =
        i32::try_from(output.stdout.len()).map_err(|_| MediaAttachmentError::SizeOverflow)?;
    Ok(PreparedMediaAttachment {
        media_kind: MediaKind::Video,
        file_name,
        content_type: format.output_content_type.to_owned(),
        file_size,
        original_bytes: output.stdout,
        small_file_name: Some(small_file_name),
        small_content_type: Some(preview_content_type.to_owned()),
        small_bytes: Some(small_bytes),
        file_meta: json!({
            "original": media_geometry_with_av(
                inspected.width,
                inspected.height,
                inspected.duration,
                inspected.frame_rate,
                inspected.bitrate,
            ),
            "small": image_geometry(
                prepared_preview.small_width,
                prepared_preview.small_height,
            ),
        }),
        blurhash: prepared_preview.blurhash,
        width: inspected.width,
        height: inspected.height,
        small_width: prepared_preview.small_width,
        small_height: prepared_preview.small_height,
    })
}

async fn prepare_audio_attachment(
    account_id: i64,
    file_name: &str,
    source_bytes: &[u8],
    demuxer: &'static str,
    format: MediaFormat,
    config: &ProcessorConfig,
    deadline: tokio::time::Instant,
) -> Result<PreparedMediaAttachment, MediaAttachmentError> {
    let output = run_ffmpeg(
        config,
        deadline,
        source_bytes,
        demuxer,
        MAX_MATRIX_LIMIT,
        &[
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-t",
            MEDIA_STOP_DURATION,
            "-c:a",
            "libmp3lame",
            "-q:a",
            "2",
            "-stats_period",
            "86400",
            "-progress",
            "pipe:2",
            "-f",
            "mp3",
            "pipe:1",
        ],
        format.input_size_limit - 1,
    )
    .await?;
    let progress_duration = parse_progress_duration(&output.stderr)?;
    if !media_container_matches(format.output_content_type, &output.stdout) {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    let output_probe = probe_media(
        config,
        &output.stdout,
        "mp3",
        MediaKind::Audio,
        true,
        deadline,
    )
    .await?;
    let inspected = inspect_normalized_audio(&output_probe, progress_duration)?;
    let file_size =
        i32::try_from(output.stdout.len()).map_err(|_| MediaAttachmentError::SizeOverflow)?;
    Ok(PreparedMediaAttachment {
        media_kind: MediaKind::Audio,
        file_name: media_file_name(
            file_name,
            format.output_content_type,
            account_id,
            source_bytes,
        ),
        content_type: format.output_content_type.to_owned(),
        file_size,
        original_bytes: output.stdout,
        small_file_name: None,
        small_content_type: format.preview_content_type.map(str::to_owned),
        small_bytes: None,
        file_meta: json!({
            "original": media_geometry_with_av(
                0,
                0,
                inspected.duration,
                None,
                inspected.bitrate,
            ),
        }),
        blurhash: None,
        width: 0,
        height: 0,
        small_width: 0,
        small_height: 0,
    })
}

#[derive(Clone, Copy, Debug)]
struct InspectedMedia {
    width: u32,
    height: u32,
    duration: f64,
    frame_rate: Option<f64>,
    bitrate: Option<u64>,
}

async fn probe_media(
    config: &ProcessorConfig,
    bytes: &[u8],
    demuxer: &'static str,
    kind: MediaKind,
    bounded_output: bool,
    deadline: tokio::time::Instant,
) -> Result<Value, MediaAttachmentError> {
    let max_pixels = match kind {
        MediaKind::Video => MEDIA_MATRIX_LIMIT,
        MediaKind::Image | MediaKind::Audio | MediaKind::Gif => MAX_MATRIX_LIMIT,
    };
    let mut arguments = [
        "-v",
        "error",
        "-protocol_whitelist",
        "pipe",
        "-threads",
        "1",
        "-max_pixels",
        &max_pixels.to_string(),
        "-f",
        demuxer,
        "-count_frames",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    if !bounded_output {
        arguments.extend([
            OsString::from("-read_intervals"),
            OsString::from("%+#36001"),
        ]);
    }
    arguments.extend(
        [
            "-i",
            "pipe:0",
            "-show_entries",
            "format=format_name,duration,bit_rate:stream=index,codec_type,codec_name,pix_fmt,width,height,avg_frame_rate,nb_frames,nb_read_frames,duration,bit_rate:stream_disposition=attached_pic",
            "-of",
            "json",
        ]
        .into_iter()
        .map(OsString::from),
    );
    let output = run_child(
        config.ffprobe.clone(),
        arguments,
        bytes.to_vec(),
        MEDIA_PROBE_OUTPUT_LIMIT,
        MEDIA_STDERR_LIMIT,
        remaining_timeout(deadline)?,
    )
    .await
    .map_err(media_child_error)?;
    if output.stdout.is_empty() {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    serde_json::from_slice(&output.stdout).map_err(|_| MediaAttachmentError::InvalidMedia)
}

async fn run_ffmpeg(
    config: &ProcessorConfig,
    deadline: tokio::time::Instant,
    input: &[u8],
    demuxer: &'static str,
    max_pixels: u64,
    output_arguments: &[&str],
    output_limit: usize,
) -> Result<ChildOutput, MediaAttachmentError> {
    let mut arguments = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-protocol_whitelist",
        "pipe",
        "-threads",
        "2",
        "-filter_threads",
        "2",
        "-filter_complex_threads",
        "2",
        "-max_pixels",
        &max_pixels.to_string(),
        "-f",
        demuxer,
        "-i",
        "pipe:0",
        "-threads",
        "2",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    arguments.extend(output_arguments.iter().map(OsString::from));
    run_child(
        config.ffmpeg.clone(),
        arguments,
        input.to_vec(),
        output_limit,
        MEDIA_STDERR_LIMIT,
        remaining_timeout(deadline)?,
    )
    .await
    .map_err(media_child_error)
}

fn remaining_timeout(deadline: tokio::time::Instant) -> Result<Duration, MediaAttachmentError> {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(MediaAttachmentError::ProcessingTimedOut)
}

fn media_child_error(error: ChildError) -> MediaAttachmentError {
    match error {
        ChildError::Unavailable => MediaAttachmentError::ProcessingUnavailable,
        ChildError::TimedOut => MediaAttachmentError::ProcessingTimedOut,
        ChildError::Overflow => MediaAttachmentError::SizeOverflow,
        ChildError::Failed => MediaAttachmentError::InvalidMedia,
    }
}

async fn run_child(
    executable: PathBuf,
    arguments: Vec<OsString>,
    input: Vec<u8>,
    stdout_limit: usize,
    stderr_limit: usize,
    timeout: Duration,
) -> Result<ChildOutput, ChildError> {
    let (cancel_sender, cancel_receiver) = tokio::sync::oneshot::channel();
    let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = supervise_child(
            executable,
            arguments,
            input,
            stdout_limit,
            stderr_limit,
            timeout,
            cancel_receiver,
        )
        .await;
        let _ = result_sender.send(result);
    });
    let result = result_receiver.await.map_err(|_| ChildError::Unavailable)?;
    drop(cancel_sender);
    result
}

#[allow(clippy::too_many_lines)]
async fn supervise_child(
    executable: PathBuf,
    arguments: Vec<OsString>,
    input: Vec<u8>,
    stdout_limit: usize,
    stderr_limit: usize,
    timeout: Duration,
    mut cancelled: tokio::sync::oneshot::Receiver<()>,
) -> Result<ChildOutput, ChildError> {
    use tokio::io::AsyncWriteExt as _;

    let mut command = tokio::process::Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| ChildError::Unavailable)?;
    let mut stdin = child.stdin.take().ok_or(ChildError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(ChildError::Unavailable)?;
    let stderr = child.stderr.take().ok_or(ChildError::Unavailable)?;

    let mut input_task = tokio::spawn(async move {
        stdin.write_all(&input).await?;
        stdin.shutdown().await
    });
    let mut stdout_task = tokio::spawn(read_bounded(stdout, stdout_limit));
    let mut stderr_task = tokio::spawn(read_bounded(stderr, stderr_limit));
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    let mut status: Option<ExitStatus> = None;
    let mut stdout_bytes = None;
    let mut stderr_bytes = None;
    let mut input_finished = false;
    loop {
        tokio::select! {
            _ = &mut cancelled => {
                abort_io_tasks(&input_task, &stdout_task, &stderr_task);
                kill_and_reap(&mut child).await;
                return Err(ChildError::Failed);
            }
            () = &mut deadline => {
                abort_io_tasks(&input_task, &stdout_task, &stderr_task);
                kill_and_reap(&mut child).await;
                return Err(ChildError::TimedOut);
            }
            result = &mut input_task, if !input_finished => {
                input_finished = true;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) if error.kind() == io::ErrorKind::BrokenPipe => {}
                    _ => {
                        abort_io_tasks(&input_task, &stdout_task, &stderr_task);
                        kill_and_reap(&mut child).await;
                        return Err(ChildError::Failed);
                    }
                }
            }
            result = &mut stdout_task, if stdout_bytes.is_none() => {
                match result {
                    Ok(Ok(bytes)) => stdout_bytes = Some(bytes),
                    Ok(Err(error)) => {
                        abort_io_tasks(&input_task, &stdout_task, &stderr_task);
                        kill_and_reap(&mut child).await;
                        return Err(error);
                    }
                    Err(_) => {
                        abort_io_tasks(&input_task, &stdout_task, &stderr_task);
                        kill_and_reap(&mut child).await;
                        return Err(ChildError::Unavailable);
                    }
                }
            }
            result = &mut stderr_task, if stderr_bytes.is_none() => {
                match result {
                    Ok(Ok(bytes)) => stderr_bytes = Some(bytes),
                    Ok(Err(error)) => {
                        abort_io_tasks(&input_task, &stdout_task, &stderr_task);
                        kill_and_reap(&mut child).await;
                        return Err(error);
                    }
                    Err(_) => {
                        abort_io_tasks(&input_task, &stdout_task, &stderr_task);
                        kill_and_reap(&mut child).await;
                        return Err(ChildError::Unavailable);
                    }
                }
            }
            result = child.wait(), if status.is_none() => {
                status = Some(result.map_err(|_| ChildError::Unavailable)?);
            }
        }
        if input_finished {
            let collected = (status.take(), stdout_bytes.take(), stderr_bytes.take());
            match collected {
                (Some(status), Some(stdout), Some(stderr)) => {
                    return if status.success() {
                        Ok(ChildOutput { stdout, stderr })
                    } else {
                        Err(ChildError::Failed)
                    };
                }
                (new_status, new_stdout, new_stderr) => {
                    status = new_status;
                    stdout_bytes = new_stdout;
                    stderr_bytes = new_stderr;
                }
            }
        }
    }
}

fn abort_io_tasks(
    input: &tokio::task::JoinHandle<io::Result<()>>,
    stdout: &tokio::task::JoinHandle<Result<Vec<u8>, ChildError>>,
    stderr: &tokio::task::JoinHandle<Result<Vec<u8>, ChildError>>,
) {
    input.abort();
    stdout.abort();
    stderr.abort();
}

async fn kill_and_reap(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

async fn read_bounded(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, ChildError> {
    use tokio::io::AsyncReadExt as _;

    let mut bytes = Vec::with_capacity(limit.min(8 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|_| ChildError::Unavailable)?;
        if read == 0 {
            return Ok(bytes);
        }
        if read > limit.saturating_sub(bytes.len()) {
            return Err(ChildError::Overflow);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn demuxer_for_content_type(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/avif" | "image/heic" | "image/heif" | "video/mp4" | "video/quicktime"
        | "audio/m4a" | "audio/x-m4a" | "audio/mp4" | "audio/3gpp" => Some("mov"),
        "video/webm" | "audio/webm" => Some("matroska"),
        "video/ogg" | "audio/ogg" | "audio/vorbis" => Some("ogg"),
        "audio/wave" | "audio/wav" | "audio/x-wav" | "audio/x-pn-wave" | "audio/vnd.wave" => {
            Some("wav")
        }
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/flac" => Some("flac"),
        "audio/aac" => Some("aac"),
        "video/x-ms-asf" => Some("asf"),
        _ => None,
    }
}

fn media_container_matches(content_type: &str, bytes: &[u8]) -> bool {
    match content_type {
        "image/avif" => iso_bmff_has_brand(bytes, &[b"avif", b"avis"]),
        "image/heic" => iso_bmff_has_brand(bytes, &[b"heic", b"heix", b"hevc", b"hevx"]),
        "image/heif" => iso_bmff_has_brand(
            bytes,
            &[b"heic", b"heix", b"hevc", b"hevx", b"mif1", b"msf1"],
        ),
        "video/mp4" => iso_bmff_has_brand(
            bytes,
            &[
                b"isom", b"iso2", b"iso3", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"avc1",
                b"dash", b"M4V ", b"MSNV", b"F4V ",
            ],
        ),
        "video/quicktime" => iso_bmff_has_brand(bytes, &[b"qt  "]),
        "audio/m4a" | "audio/x-m4a" => iso_bmff_has_brand(bytes, &[b"M4A ", b"M4B ", b"M4P "]),
        "audio/mp4" => iso_bmff_has_brand(
            bytes,
            &[
                b"isom", b"iso2", b"iso3", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"M4A ",
                b"M4B ", b"M4P ",
            ],
        ),
        "audio/3gpp" => iso_bmff_brands(bytes)
            .is_some_and(|brands| brands.iter().any(|brand| brand.starts_with(b"3gp"))),
        "video/webm" | "audio/webm" => ebml_document_type(bytes) == Some(b"webm".as_slice()),
        "video/ogg" | "audio/ogg" | "audio/vorbis" => bytes.starts_with(b"OggS"),
        "audio/wave" | "audio/wav" | "audio/x-wav" | "audio/x-pn-wave" | "audio/vnd.wave" => {
            bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE".as_slice())
        }
        "audio/mpeg" | "audio/mp3" => {
            bytes.starts_with(b"ID3")
                || bytes
                    .get(..2)
                    .is_some_and(|header| header[0] == 0xff && header[1] & 0xe0 == 0xe0)
        }
        "audio/flac" => bytes.starts_with(b"fLaC"),
        "audio/aac" => bytes
            .get(..2)
            .is_some_and(|header| header[0] == 0xff && header[1] & 0xf6 == 0xf0),
        "video/x-ms-asf" => bytes.starts_with(&[
            0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62,
            0xce, 0x6c,
        ]),
        _ => false,
    }
}

fn iso_bmff_has_brand(bytes: &[u8], expected: &[&[u8; 4]]) -> bool {
    iso_bmff_brands(bytes).is_some_and(|brands| {
        brands.iter().any(|brand| {
            expected
                .iter()
                .any(|expected| *brand == expected.as_slice())
        })
    })
}

fn iso_bmff_brands(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    let mut offset = 0_usize;
    while offset.checked_add(8)? <= bytes.len().min(4 * 1024) {
        let size = u32::from_be_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?) as usize;
        if size < 8 || offset.checked_add(size)? > bytes.len() {
            return None;
        }
        if bytes.get(offset + 4..offset + 8)? == b"ftyp" {
            if size < 16 || !(size - 16).is_multiple_of(4) {
                return None;
            }
            let mut brands = vec![bytes.get(offset + 8..offset + 12)?];
            brands.extend(bytes.get(offset + 16..offset + size)?.chunks_exact(4));
            return Some(brands);
        }
        offset += size;
    }
    None
}

fn ebml_document_type(bytes: &[u8]) -> Option<&[u8]> {
    let (header_id, header_id_length) = ebml_vint(bytes, 0, true)?;
    if header_id != 0x1a45_dfa3 {
        return None;
    }
    let (header_size, header_size_length) = ebml_vint(bytes, header_id_length, false)?;
    let mut offset = header_id_length.checked_add(header_size_length)?;
    let header_end = offset.checked_add(header_size)?;
    if header_end > bytes.len() {
        return None;
    }
    while offset < header_end {
        let (element_id, id_length) = ebml_vint(bytes, offset, true)?;
        offset = offset.checked_add(id_length)?;
        let (element_size, size_length) = ebml_vint(bytes, offset, false)?;
        offset = offset.checked_add(size_length)?;
        let element_end = offset.checked_add(element_size)?;
        if element_end > header_end {
            return None;
        }
        if element_id == 0x4282 {
            return bytes.get(offset..element_end);
        }
        offset = element_end;
    }
    None
}

fn ebml_vint(bytes: &[u8], offset: usize, retain_marker: bool) -> Option<(usize, usize)> {
    let first = *bytes.get(offset)?;
    let length = (1..=8).find(|count| first & (0x80 >> (count - 1)) != 0)?;
    let marker_mask = 0x80 >> (length - 1);
    let mut value = if retain_marker {
        usize::from(first)
    } else {
        usize::from(first & (marker_mask - 1))
    };
    for byte in bytes.get(offset + 1..offset.checked_add(length)?)? {
        value = value.checked_mul(256)?.checked_add(usize::from(*byte))?;
    }
    Some((value, length))
}

fn inspect_probe(
    probe: &Value,
    kind: MediaKind,
    declared_content_type: &str,
) -> Result<InspectedMedia, MediaAttachmentError> {
    let expected_demuxer = demuxer_for_content_type(declared_content_type)
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    validate_container(probe, expected_demuxer)?;
    let streams = probe["streams"]
        .as_array()
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    let duration = probe_duration(probe)?;
    let bitrate = probe_bitrate(probe)?;

    match kind {
        MediaKind::Image => {
            if duration.is_some_and(|value| value > 0.0) {
                return Err(MediaAttachmentError::InvalidMedia);
            }
            inspect_modern_image(streams, declared_content_type, bitrate)
        }
        MediaKind::Video => inspect_video_streams(streams, duration, bitrate, false),
        MediaKind::Audio => inspect_audio_streams(streams, duration, bitrate, false),
        MediaKind::Gif => Err(MediaAttachmentError::InvalidMedia),
    }
}

fn inspect_modern_image(
    streams: &[Value],
    declared_content_type: &str,
    bitrate: Option<u64>,
) -> Result<InspectedMedia, MediaAttachmentError> {
    if streams.is_empty() {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    let expected_codec = match declared_content_type {
        "image/avif" => "av1",
        "image/heic" | "image/heif" => "hevc",
        _ => return Err(MediaAttachmentError::InvalidMedia),
    };
    let mut primary_dimensions = None;
    for stream in streams {
        if stream["codec_type"].as_str() != Some("video")
            || stream["codec_name"].as_str() != Some(expected_codec)
            || stream_attached_pic(stream)?
            || probe_frame_count(stream)? != 1
            || optional_probe_number(&stream["duration"])?.is_some_and(|value| value > 0.0)
        {
            return Err(MediaAttachmentError::InvalidMedia);
        }
        let dimensions = probe_dimensions(stream, MAX_MATRIX_LIMIT)?;
        primary_dimensions.get_or_insert(dimensions);
    }
    let (width, height) = primary_dimensions.ok_or(MediaAttachmentError::InvalidMedia)?;
    Ok(InspectedMedia {
        width,
        height,
        duration: 0.0,
        frame_rate: None,
        bitrate,
    })
}

fn inspect_video_streams(
    streams: &[Value],
    duration: Option<f64>,
    bitrate: Option<u64>,
    normalized: bool,
) -> Result<InspectedMedia, MediaAttachmentError> {
    let videos = streams
        .iter()
        .filter(|stream| stream["codec_type"] == "video")
        .collect::<Vec<_>>();
    let audios = streams
        .iter()
        .filter(|stream| stream["codec_type"] == "audio")
        .collect::<Vec<_>>();
    if videos.len() != 1 || audios.len() > 1 || streams.len() != videos.len() + audios.len() {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    if stream_attached_pic(videos[0])? {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    let video = videos[0];
    if normalized
        && (video["codec_name"].as_str() != Some("h264")
            || video["pix_fmt"].as_str() != Some("yuv420p")
            || audios
                .first()
                .is_some_and(|audio| audio["codec_name"].as_str() != Some("aac")))
    {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    let (width, height) = probe_dimensions(video, MEDIA_MATRIX_LIMIT)?;
    let frame_rate = match probe_rational(&video["avg_frame_rate"]) {
        Ok(rate) if rate > 0.0 && rate <= MEDIA_MAX_FRAME_RATE => Some(rate),
        Err(_) if !normalized && video["avg_frame_rate"].as_str() == Some("0/0") => None,
        _ => return Err(MediaAttachmentError::InvalidMedia),
    };
    let frames = probe_frame_count(video)?;
    if frames == 0 || frames > MEDIA_MAX_FRAMES {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    #[allow(clippy::cast_precision_loss)]
    let estimated_duration = frame_rate.map(|rate| frames as f64 / rate);
    let duration = duration.or(estimated_duration);
    if duration.is_some_and(|value| !(value > 0.0 && value <= MEDIA_MAX_DURATION_SECONDS))
        || (normalized && duration.is_none())
    {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    Ok(InspectedMedia {
        width,
        height,
        duration: duration.unwrap_or_default(),
        frame_rate,
        bitrate,
    })
}

fn inspect_audio_streams(
    streams: &[Value],
    duration: Option<f64>,
    bitrate: Option<u64>,
    normalized: bool,
) -> Result<InspectedMedia, MediaAttachmentError> {
    let audio_count = streams
        .iter()
        .filter(|stream| stream["codec_type"] == "audio")
        .count();
    if audio_count == 0 || (normalized && audio_count != 1) {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    for stream in streams {
        match stream["codec_type"].as_str() {
            Some("audio") => {
                if normalized && stream["codec_name"].as_str() != Some("mp3") {
                    return Err(MediaAttachmentError::InvalidMedia);
                }
            }
            Some("video") if stream_attached_pic(stream)? => {
                if probe_frame_count(stream)? != 1 {
                    return Err(MediaAttachmentError::InvalidMedia);
                }
                probe_dimensions(stream, MAX_MATRIX_LIMIT)?;
            }
            _ => return Err(MediaAttachmentError::InvalidMedia),
        }
    }
    if let Some(duration) = duration
        && (!(duration > 0.0 && duration <= MEDIA_MAX_DURATION_SECONDS))
    {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    Ok(InspectedMedia {
        width: 0,
        height: 0,
        duration: duration.unwrap_or_default(),
        frame_rate: None,
        bitrate,
    })
}

fn inspect_normalized_video(probe: &Value) -> Result<InspectedMedia, MediaAttachmentError> {
    validate_container(probe, "mov")?;
    let streams = probe["streams"]
        .as_array()
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    inspect_video_streams(streams, probe_duration(probe)?, probe_bitrate(probe)?, true)
}

fn inspect_normalized_audio(
    probe: &Value,
    progress_duration: Option<f64>,
) -> Result<InspectedMedia, MediaAttachmentError> {
    validate_container(probe, "mp3")?;
    let streams = probe["streams"]
        .as_array()
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    let mut inspected = inspect_audio_streams(
        streams,
        probe_duration(probe)?.or(progress_duration),
        probe_bitrate(probe)?,
        true,
    )?;
    if inspected.duration <= 0.0 || inspected.duration > MEDIA_MAX_DURATION_SECONDS {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    inspected.duration = probe_duration(probe)?
        .or(progress_duration)
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    Ok(inspected)
}

fn validate_container(probe: &Value, demuxer: &str) -> Result<(), MediaAttachmentError> {
    let actual = probe["format"]["format_name"]
        .as_str()
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    let expected = match demuxer {
        "mov" => "mov,mp4,m4a,3gp,3g2,mj2",
        "matroska" => "matroska,webm",
        "ogg" => "ogg",
        "wav" => "wav",
        "mp3" => "mp3",
        "flac" => "flac",
        "aac" => "aac",
        "asf" => "asf",
        _ => return Err(MediaAttachmentError::InvalidMedia),
    };
    (actual == expected)
        .then_some(())
        .ok_or(MediaAttachmentError::InvalidMedia)
}

fn probe_dimensions(stream: &Value, matrix_limit: u64) -> Result<(u32, u32), MediaAttachmentError> {
    let width = probe_u64(&stream["width"])?
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    let height = probe_u64(&stream["height"])?
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    if u64::from(width) * u64::from(height) > matrix_limit {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    Ok((width, height))
}

fn probe_duration(probe: &Value) -> Result<Option<f64>, MediaAttachmentError> {
    let format_duration = optional_probe_number(&probe["format"]["duration"])?;
    if format_duration.is_some() {
        return Ok(format_duration);
    }
    for stream in probe["streams"]
        .as_array()
        .ok_or(MediaAttachmentError::InvalidMedia)?
    {
        if let Some(duration) = optional_probe_number(&stream["duration"])? {
            return Ok(Some(duration));
        }
    }
    Ok(None)
}

fn probe_bitrate(probe: &Value) -> Result<Option<u64>, MediaAttachmentError> {
    let format_bitrate = probe_u64(&probe["format"]["bit_rate"])?;
    if format_bitrate.is_some() {
        return Ok(format_bitrate);
    }
    for stream in probe["streams"]
        .as_array()
        .ok_or(MediaAttachmentError::InvalidMedia)?
    {
        if let Some(bitrate) = probe_u64(&stream["bit_rate"])? {
            return Ok(Some(bitrate));
        }
    }
    Ok(None)
}

fn optional_probe_number(value: &Value) -> Result<Option<f64>, MediaAttachmentError> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(Some)
        .ok_or(MediaAttachmentError::InvalidMedia)
}

fn probe_u64(value: &Value) -> Result<Option<u64>, MediaAttachmentError> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<u64>().ok()))
        .map(Some)
        .ok_or(MediaAttachmentError::InvalidMedia)
}

fn probe_rational(value: &Value) -> Result<f64, MediaAttachmentError> {
    let value = value.as_str().ok_or(MediaAttachmentError::InvalidMedia)?;
    let (numerator, denominator) = value
        .split_once('/')
        .ok_or(MediaAttachmentError::InvalidMedia)?;
    let numerator = numerator
        .parse::<f64>()
        .map_err(|_| MediaAttachmentError::InvalidMedia)?;
    let denominator = denominator
        .parse::<f64>()
        .map_err(|_| MediaAttachmentError::InvalidMedia)?;
    let result = numerator / denominator;
    if denominator == 0.0 || !result.is_finite() {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    Ok(result)
}

fn probe_frame_count(stream: &Value) -> Result<u64, MediaAttachmentError> {
    probe_u64(&stream["nb_read_frames"])?
        .or(probe_u64(&stream["nb_frames"])?)
        .ok_or(MediaAttachmentError::InvalidMedia)
}

fn stream_attached_pic(stream: &Value) -> Result<bool, MediaAttachmentError> {
    match &stream["disposition"]["attached_pic"] {
        Value::Null => Ok(false),
        Value::Number(value) => match value.as_u64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(MediaAttachmentError::InvalidMedia),
        },
        _ => Err(MediaAttachmentError::InvalidMedia),
    }
}

fn parse_progress_duration(stderr: &[u8]) -> Result<Option<f64>, MediaAttachmentError> {
    let text = std::str::from_utf8(stderr).map_err(|_| MediaAttachmentError::InvalidMedia)?;
    let mut duration = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("out_time_us=") {
            let micros = value
                .parse::<u64>()
                .map_err(|_| MediaAttachmentError::InvalidMedia)?;
            #[allow(clippy::cast_precision_loss)]
            let seconds = micros as f64 / 1_000_000.0;
            duration = Some(seconds);
        }
    }
    if duration.is_some_and(|value| value > MEDIA_MAX_DURATION_SECONDS) {
        return Err(MediaAttachmentError::InvalidMedia);
    }
    Ok(duration)
}

fn media_geometry_with_av(
    width: u32,
    height: u32,
    duration: f64,
    frame_rate: Option<f64>,
    bitrate: Option<u64>,
) -> Value {
    let mut value = if width > 0 && height > 0 {
        image_geometry(width, height)
    } else {
        json!({})
    };
    let object = value.as_object_mut().expect("media metadata is an object");
    object.insert("duration".to_owned(), json!(duration));
    if let Some(frame_rate) = frame_rate {
        object.insert("frame_rate".to_owned(), json!(frame_rate));
    }
    if let Some(bitrate) = bitrate {
        object.insert("bitrate".to_owned(), json!(bitrate));
    }
    value
}

/// Validates a federated custom emoji and creates its static PNG style.
///
/// # Errors
///
/// Returns an error for unsupported, oversized, malformed, or mismatched image data.
pub fn prepare_custom_emoji(
    emoji_id: i64,
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<PreparedCustomEmoji, MediaAttachmentError> {
    if !matches!(content_type, "image/png" | "image/gif" | "image/webp") {
        return Err(MediaAttachmentError::UnsupportedContentType);
    }
    if bytes.len() >= 256 * 1024 {
        return Err(MediaAttachmentError::TooLarge);
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| MediaAttachmentError::InvalidImage)?;
    let format = reader.format().ok_or(MediaAttachmentError::InvalidImage)?;
    if image_format_for_content_type(content_type) != Some(format) {
        return Err(MediaAttachmentError::InvalidImage);
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| MediaAttachmentError::InvalidImage)?;
    validate_image_matrix(content_type, width, height)
        .map_err(|_| MediaAttachmentError::InvalidImage)?;
    let image = if format == ImageFormat::Gif {
        let mut decoder =
            GifDecoder::new(Cursor::new(bytes)).map_err(|_| MediaAttachmentError::InvalidImage)?;
        decoder
            .set_limits(image_limits())
            .map_err(|_| MediaAttachmentError::InvalidImage)?;
        let mut frames = decoder.into_frames();
        let frame = frames
            .next()
            .ok_or(MediaAttachmentError::InvalidImage)?
            .map_err(|_| MediaAttachmentError::InvalidImage)?;
        let (frame_width, frame_height) = frame.buffer().dimensions();
        let mut decoded_pixels = u64::from(frame_width) * u64::from(frame_height);
        for (index, frame) in frames.enumerate() {
            if index >= CUSTOM_EMOJI_GIF_MAX_FRAMES - 1 {
                return Err(MediaAttachmentError::InvalidImage);
            }
            let frame = frame.map_err(|_| MediaAttachmentError::InvalidImage)?;
            let (frame_width, frame_height) = frame.buffer().dimensions();
            decoded_pixels = decoded_pixels
                .checked_add(u64::from(frame_width) * u64::from(frame_height))
                .filter(|pixels| *pixels <= CUSTOM_EMOJI_GIF_MAX_PIXELS)
                .ok_or(MediaAttachmentError::InvalidImage)?;
        }
        image::DynamicImage::ImageRgba8(frame.into_buffer())
    } else {
        let mut reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|_| MediaAttachmentError::InvalidImage)?;
        reader.limits(image_limits());
        reader
            .decode()
            .map_err(|_| MediaAttachmentError::InvalidImage)?
    };
    let static_bytes =
        encode_image(&image, ImageFormat::Png).map_err(|_| MediaAttachmentError::InvalidImage)?;
    let file_name = media_file_name(file_name, content_type, emoji_id, bytes);
    let file_size = i32::try_from(bytes.len()).map_err(|_| MediaAttachmentError::SizeOverflow)?;
    Ok(PreparedCustomEmoji {
        file_name,
        content_type: content_type.to_owned(),
        file_size,
        original_bytes: bytes.to_vec(),
        static_bytes,
    })
}

fn media_blurhash(image: &image::DynamicImage) -> Option<String> {
    let thumbnail = image.resize(100, 100, FilterType::Lanczos3).to_rgba8();
    let (width, height) = thumbnail.dimensions();
    blurhash::encode(4, 4, width, height, thumbnail.as_raw()).ok()
}

fn resize_media_image(
    image: image::DynamicImage,
    input_width: u32,
    input_height: u32,
) -> image::DynamicImage {
    resize_media_image_to_limit(image, input_width, input_height, 230_400)
}

fn resize_media_image_to_limit(
    image: image::DynamicImage,
    input_width: u32,
    input_height: u32,
    pixel_limit: u64,
) -> image::DynamicImage {
    let matrix = u64::from(input_width) * u64::from(input_height);
    if matrix <= pixel_limit {
        return image;
    }
    #[allow(clippy::cast_precision_loss)]
    let scale = (pixel_limit as f64 / matrix as f64).sqrt();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let width = (f64::from(input_width) * scale).round().max(1.0) as u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let height = (f64::from(input_height) * scale).round().max(1.0) as u32;
    image.resize_exact(width, height, FilterType::Lanczos3)
}

fn image_geometry(width: u32, height: u32) -> Value {
    json!({
        "width": width,
        "height": height,
        "size": format!("{width}x{height}"),
        "aspect": f64::from(width) / f64::from(height),
    })
}

fn image_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_alloc = Some(IMAGE_MAX_ALLOC);
    limits
}

fn validate_image_matrix(
    content_type: &str,
    width: u32,
    height: u32,
) -> Result<(), AccountMediaError> {
    let matrix = u64::from(width) * u64::from(height);
    let limit = if content_type == "image/gif" {
        GIF_MATRIX_LIMIT
    } else {
        MAX_MATRIX_LIMIT
    };
    (matrix <= limit)
        .then_some(())
        .ok_or(AccountMediaError::InvalidImage)
}

fn transform_profile_image(
    image: image::DynamicImage,
    attachment: PaperclipAttachment,
    input_width: u32,
    input_height: u32,
) -> image::DynamicImage {
    match attachment {
        PaperclipAttachment::AccountAvatar => image.resize_to_fill(400, 400, FilterType::Lanczos3),
        PaperclipAttachment::AccountHeader => {
            let matrix = u64::from(input_width) * u64::from(input_height);
            if matrix <= 750_000 {
                image
            } else {
                #[allow(clippy::cast_precision_loss)]
                let scale = (750_000_f64 / matrix as f64).sqrt();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let width = (f64::from(input_width) * scale).round().max(1.0) as u32;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let height = (f64::from(input_height) * scale).round().max(1.0) as u32;
                image.resize_exact(width, height, FilterType::Lanczos3)
            }
        }
        _ => image,
    }
}

fn process_gif(
    bytes: &[u8],
    attachment: PaperclipAttachment,
    input_width: u32,
    input_height: u32,
) -> Result<ProcessedGif, AccountMediaError> {
    let mut first_decoder =
        GifDecoder::new(Cursor::new(bytes)).map_err(|_| AccountMediaError::InvalidImage)?;
    first_decoder
        .set_limits(image_limits())
        .map_err(|_| AccountMediaError::InvalidImage)?;
    let first = first_decoder
        .into_frames()
        .next()
        .ok_or(AccountMediaError::InvalidImage)?
        .map_err(|_| AccountMediaError::InvalidImage)?;
    let first = transform_gif_frame(first, attachment, input_width, input_height);
    let (width, height) = first.buffer().dimensions();
    let static_bytes = encode_image(
        &image::DynamicImage::ImageRgba8(first.buffer().clone()),
        ImageFormat::Png,
    )?;

    let mut decoder =
        GifDecoder::new(Cursor::new(bytes)).map_err(|_| AccountMediaError::InvalidImage)?;
    decoder
        .set_limits(image_limits())
        .map_err(|_| AccountMediaError::InvalidImage)?;
    let mut original_bytes = BoundedImageBytes(Vec::new());
    {
        let mut encoder = GifEncoder::new(&mut original_bytes);
        for (index, frame) in decoder.into_frames().enumerate() {
            let frames = u64::try_from(index + 1).map_err(|_| AccountMediaError::TooLarge)?;
            if frames > 256
                || frames * u64::from(input_width) * u64::from(input_height) > 16_777_216
                || frames * u64::from(width) * u64::from(height) > 67_108_864
            {
                return Err(AccountMediaError::TooLarge);
            }
            let frame = frame.map_err(|_| AccountMediaError::InvalidImage)?;
            let frame = transform_gif_frame(frame, attachment, input_width, input_height);
            encoder
                .encode_frame(frame)
                .map_err(|_| AccountMediaError::InvalidImage)?;
        }
    }
    Ok((original_bytes.0, Some(static_bytes), width, height))
}

/// Bound the encoder while it writes, rather than checking a potentially huge Vec afterward.
struct BoundedImageBytes(Vec<u8>);
impl Write for BoundedImageBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > ACCOUNT_MEDIA_LIMIT.saturating_sub(self.0.len()) {
            return Err(io::Error::other("profile image output limit exceeded"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn transform_gif_frame(
    frame: Frame,
    attachment: PaperclipAttachment,
    input_width: u32,
    input_height: u32,
) -> Frame {
    let delay: Delay = frame.delay();
    let image = image::DynamicImage::ImageRgba8(frame.into_buffer());
    let image = transform_profile_image(image, attachment, input_width, input_height);
    Frame::from_parts(image.into_rgba8(), 0, 0, delay)
}

fn image_format_for_content_type(content_type: &str) -> Option<ImageFormat> {
    match content_type {
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/png" => Some(ImageFormat::Png),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

fn encode_image(
    image: &image::DynamicImage,
    format: ImageFormat,
) -> Result<Vec<u8>, AccountMediaError> {
    let mut output = Cursor::new(Vec::new());
    if format == ImageFormat::Jpeg {
        image
            .write_with_encoder(JpegEncoder::new_with_quality(&mut output, 90))
            .map_err(|_| AccountMediaError::InvalidImage)?;
    } else {
        image
            .write_to(&mut output, format)
            .map_err(|_| AccountMediaError::InvalidImage)?;
    }
    Ok(output.into_inner())
}

fn media_file_name(file_name: &str, content_type: &str, account_id: i64, bytes: &[u8]) -> String {
    let candidate = file_name.rsplit(['/', '\\']).next().unwrap_or_default();
    let original_extension = candidate
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    let extension = match content_type {
        "image/jpeg" => match original_extension.as_deref() {
            Some("jpg" | "jpeg") => original_extension.unwrap(),
            _ => "jpg".to_owned(),
        },
        "image/png" => "png".to_owned(),
        "image/gif" => "gif".to_owned(),
        "image/webp" => "webp".to_owned(),
        "video/mp4" => "mp4".to_owned(),
        "audio/mpeg" => "mp3".to_owned(),
        _ => "bin".to_owned(),
    };
    let mut digest = Sha256::new();
    digest.update(account_id.to_le_bytes());
    digest.update(bytes);
    let digest = format!("{:x}", digest.finalize());
    format!("{}.{extension}", &digest[..16])
}

impl PaperclipMetadata {
    #[must_use]
    pub fn relative_path(&self, style: &str) -> Option<String> {
        let partition = partitioned_id(self.id)?;
        let file_name = self.style_file_name(style)?;
        let cache = if self.cache_prefix() { "cache/" } else { "" };
        Some(format!(
            "{cache}{}/{partition}/{style}/{file_name}",
            self.attachment.path()
        ))
    }

    /// MIME of a media file's served style, without changing original metadata.
    #[must_use]
    pub(crate) fn media_file_content_type(&self, style: &str) -> Option<&str> {
        if self.attachment != PaperclipAttachment::MediaFile {
            return None;
        }
        let content_type = self.content_type.as_deref()?;
        let format = media_format(content_type)?;
        match style {
            "original" => Some(content_type),
            "small" => format.preview_content_type,
            _ => None,
        }
    }

    fn cache_prefix(&self) -> bool {
        self.attachment.permits_cache()
            && self.remote
            && self.storage_schema_version.unwrap_or_default() >= 1
    }

    fn style_file_name(&self, style: &str) -> Option<String> {
        if !safe_component(&self.file_name) {
            return None;
        }
        if style == "original" {
            return Some(self.file_name.clone());
        }
        let png = || derivative_file_name(&self.file_name, "png");
        match self.attachment {
            PaperclipAttachment::AccountAvatar | PaperclipAttachment::AccountHeader => {
                (style == "static" && self.content_type.as_deref() == Some("image/gif"))
                    .then(png)
                    .flatten()
            }
            PaperclipAttachment::CustomEmojiImage
            | PaperclipAttachment::PreviewCardProviderIcon => {
                (style == "static").then(png).flatten()
            }
            PaperclipAttachment::MediaFile if style == "small" => {
                match self.media_file_content_type(style)? {
                    content_type if Some(content_type) == self.content_type.as_deref() => {
                        Some(self.file_name.clone())
                    }
                    "image/png" => png(),
                    "image/jpeg" => derivative_file_name(&self.file_name, "jpeg"),
                    _ => None,
                }
            }
            PaperclipAttachment::SiteUploadFile => {
                let styles = match self.variant.as_deref()? {
                    "app_icon" => APP_ICON_STYLES,
                    "favicon" => FAVICON_STYLES,
                    "thumbnail" => THUMBNAIL_STYLES,
                    "mascot" => return (style == "mascot").then(|| self.file_name.clone()),
                    _ => return None,
                };
                styles.contains(&style).then(png).flatten()
            }
            PaperclipAttachment::MediaFile
            | PaperclipAttachment::MediaThumbnail
            | PaperclipAttachment::PreviewCardImage => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaperclipPath {
    relative_path: String,
    attachment: PaperclipAttachment,
    id: i64,
}

#[derive(Clone, Debug)]
pub struct PaperclipRoot {
    directory: Arc<OwnedFd>,
    #[cfg(feature = "test-support")]
    write_fault: Option<Arc<PaperclipWriteFault>>,
    #[cfg(feature = "test-support")]
    commit_fault: Option<Arc<PaperclipCommitFault>>,
    #[cfg(feature = "test-support")]
    remove_fault: Option<Arc<PaperclipRemoveFault>>,
    #[cfg(feature = "test-support")]
    directory_sync_fault: Option<Arc<PaperclipDirectorySyncFault>>,
}

impl PaperclipRoot {
    /// Opens a canonical absolute media root without following symlinks in any component.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if `path` is not a clean absolute directory or cannot be opened
    /// without following symlinks.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let relative = path.strip_prefix("/").map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Paperclip root must be absolute",
            )
        })?;
        if !safe_relative_path(relative) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Paperclip root must not be the filesystem root",
            ));
        }
        let filesystem_root = open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let directory = openat2(
            &filesystem_root,
            relative,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )?;
        if !FileType::from_raw_mode(fstat(&directory)?.st_mode).is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Paperclip root is not a directory",
            ));
        }
        Ok(Self {
            directory: Arc::new(directory),
            #[cfg(feature = "test-support")]
            write_fault: None,
            #[cfg(feature = "test-support")]
            commit_fault: None,
            #[cfg(feature = "test-support")]
            remove_fault: None,
            #[cfg(feature = "test-support")]
            directory_sync_fault: None,
        })
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_write_fault(mut self, fault: PaperclipWriteFault) -> Self {
        self.write_fault = Some(Arc::new(fault));
        self
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_commit_fault(mut self, fault: PaperclipCommitFault) -> Self {
        self.commit_fault = Some(Arc::new(fault));
        self
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_remove_fault(mut self, fault: PaperclipRemoveFault) -> Self {
        self.remove_fault = Some(Arc::new(fault));
        self
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_directory_sync_fault(mut self, fault: PaperclipDirectorySyncFault) -> Self {
        self.directory_sync_fault = Some(Arc::new(fault));
        self
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn take_commit_before_fault(&self) -> bool {
        self.commit_fault
            .as_ref()
            .is_some_and(|fault| fault.take_before_commit())
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn take_commit_after_fault(&self) -> bool {
        self.commit_fault
            .as_ref()
            .is_some_and(|fault| fault.take_after_commit())
    }

    /// Opens a regular file beneath this root without following any symlink component or updating
    /// its access time.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the path is unsafe, the target cannot be opened read-only without
    /// atime updates, or the target is not a regular file.
    pub fn open_file(&self, relative_path: &Path) -> std::io::Result<File> {
        if !safe_relative_path(relative_path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Paperclip paths must be clean and relative",
            ));
        }
        let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        let resolve =
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
        let file = openat2(
            self.directory.as_ref(),
            relative_path,
            flags | OFlags::NOATIME,
            Mode::empty(),
            resolve,
        )
        .or_else(|error| {
            if error == rustix::io::Errno::PERM {
                openat2(
                    self.directory.as_ref(),
                    relative_path,
                    flags,
                    Mode::empty(),
                    resolve,
                )
            } else {
                Err(error)
            }
        })?;
        let stat = fstat(&file)?;
        if !FileType::from_raw_mode(stat.st_mode).is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Paperclip target is not a regular file",
            ));
        }
        Ok(File::from(file))
    }

    /// Writes a regular file beneath this root without following any symlink component.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the path is unsafe, its parent cannot be created securely, or the
    /// file cannot be written.
    pub fn write_file(&self, relative_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if !safe_relative_path(relative_path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Paperclip paths must be clean and relative",
            ));
        }
        #[cfg(feature = "test-support")]
        if self
            .write_fault
            .as_ref()
            .is_some_and(|fault| fault.should_fail())
        {
            return Err(io::Error::from(io::ErrorKind::StorageFull));
        }
        let file_name = relative_path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Paperclip file name is missing",
            )
        })?;
        let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
        let directory = self.open_directory(parent, true)?;
        let file = match openat2(
            &directory,
            file_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_bits_retain(0o644),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        ) {
            Ok(file) => file,
            Err(Errno::EXIST) => return self.open_file(relative_path).map(|_| ()),
            Err(error) => return Err(error.into()),
        };
        let mut file = File::from(file);
        let result = fchmod(&file, Mode::from_bits_retain(0o644))
            .map_err(io::Error::from)
            .and_then(|()| file.write_all(bytes))
            .and_then(|()| file.sync_all());
        if let Err(error) = result {
            drop(file);
            let _ = unlinkat(&directory, file_name, AtFlags::empty());
            return Err(error);
        }
        self.sync_directory(&directory)?;
        Ok(())
    }

    /// Removes a regular file beneath this root without following any symlink component.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the path is unsafe or its parent cannot be opened securely.
    pub fn remove_file(&self, relative_path: &Path) -> std::io::Result<()> {
        if !safe_relative_path(relative_path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Paperclip paths must be clean and relative",
            ));
        }
        #[cfg(feature = "test-support")]
        if self
            .remove_fault
            .as_ref()
            .is_some_and(|fault| fault.should_fail())
        {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        let file_name = relative_path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Paperclip file name is missing",
            )
        })?;
        let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
        let directory = match self.open_directory(parent, false) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        match unlinkat(&directory, file_name, AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => self.sync_directory(&directory),
            Err(error) => Err(error.into()),
        }
    }

    fn open_directory(&self, relative_path: &Path, create: bool) -> std::io::Result<OwnedFd> {
        if !safe_relative_path(relative_path) && !relative_path.as_os_str().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Paperclip paths must be clean and relative",
            ));
        }
        let mut directory = self.directory.try_clone()?;
        for component in relative_path.components() {
            let Component::Normal(component) = component else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Paperclip paths must be clean and relative",
                ));
            };
            if create {
                match mkdirat(&directory, component, Mode::from_bits_retain(0o755)) {
                    Ok(()) | Err(Errno::EXIST) => self.sync_directory(&directory)?,
                    Err(error) => return Err(error.into()),
                }
            }
            directory = openat2(
                &directory,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )?;
            if !FileType::from_raw_mode(fstat(&directory)?.st_mode).is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Paperclip parent is not a directory",
                ));
            }
        }
        Ok(directory)
    }

    fn sync_directory(&self, directory: &OwnedFd) -> io::Result<()> {
        #[cfg(feature = "test-support")]
        if self
            .directory_sync_fault
            .as_ref()
            .is_some_and(|fault| fault.should_fail())
        {
            return Err(io::Error::other("injected directory sync failure"));
        }
        fsync(directory).map_err(Into::into)
    }
}

/// Writes a prepared media attachment beneath its exact Paperclip paths.
///
/// The original is removed when writing the derivative fails, so callers can safely retry the
/// database update without exposing a half-written attachment.
///
/// # Errors
///
/// Returns an I/O error when the generated Paperclip paths cannot be created or written safely.
pub fn write_prepared_media(
    root: &PaperclipRoot,
    metadata: &PaperclipMetadata,
    prepared: &PreparedMediaAttachment,
) -> std::io::Result<Vec<String>> {
    let original = metadata.relative_path("original").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid media path")
    })?;
    write_prepared_file(root, Path::new(&original), &prepared.original_bytes)?;
    let mut paths = vec![original];
    if let Some(small_bytes) = prepared.small_bytes.as_ref() {
        let small = metadata.relative_path("small").ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid media path")
        })?;
        if let Err(error) = write_prepared_file(root, Path::new(&small), small_bytes) {
            for path in &paths {
                let _ = root.remove_file(Path::new(path));
            }
            let _ = root.remove_file(Path::new(&small));
            return Err(error);
        }
        paths.push(small);
    }
    Ok(paths)
}

/// Writes the original and optional static derivative of a prepared profile image.
///
/// # Errors
/// Returns an I/O error if safe Paperclip writes fail.
pub fn write_prepared_account_media(
    root: &PaperclipRoot,
    metadata: &PaperclipMetadata,
    prepared: &PreparedAccountMedia,
) -> std::io::Result<Vec<String>> {
    let original = metadata.relative_path("original").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid media path")
    })?;
    let mut paths = vec![original];
    write_prepared_file(root, Path::new(&paths[0]), &prepared.original_bytes)?;
    if let Some(static_bytes) = prepared.static_bytes.as_ref() {
        let static_path = metadata.relative_path("static").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid static profile path")
        })?;
        // The original may already be referenced by a committed row. The caller's
        // row-aware reconciliation, not a partial-write error, decides what to unlink.
        write_prepared_file(root, Path::new(&static_path), static_bytes)?;
        paths.push(static_path);
    }
    Ok(paths)
}

/// Writes a prepared custom emoji beneath its Paperclip original and static paths.
///
/// # Errors
///
/// Returns an I/O error if either file cannot be written and verified safely.
pub fn write_prepared_custom_emoji(
    root: &PaperclipRoot,
    metadata: &PaperclipMetadata,
    prepared: &PreparedCustomEmoji,
) -> std::io::Result<Vec<String>> {
    let original = metadata.relative_path("original").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid emoji path")
    })?;
    let static_path = metadata.relative_path("static").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid emoji path")
    })?;
    write_prepared_file(root, Path::new(&original), &prepared.original_bytes)?;
    if let Err(error) = write_prepared_file(root, Path::new(&static_path), &prepared.static_bytes) {
        let _ = root.remove_file(Path::new(&original));
        let _ = root.remove_file(Path::new(&static_path));
        return Err(error);
    }
    Ok(vec![original, static_path])
}

fn write_prepared_file(root: &PaperclipRoot, relative_path: &Path, bytes: &[u8]) -> io::Result<()> {
    root.write_file(relative_path, bytes)?;
    let mut existing = Vec::new();
    root.open_file(relative_path)?.read_to_end(&mut existing)?;
    if existing == bytes {
        return Ok(());
    }
    root.remove_file(relative_path)?;
    root.write_file(relative_path, bytes)?;
    let mut rewritten = Vec::new();
    root.open_file(relative_path)?.read_to_end(&mut rewritten)?;
    if rewritten == bytes {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Paperclip file contents changed while writing",
        ))
    }
}

impl PaperclipPath {
    #[must_use]
    pub fn authorizes(&self, metadata: &PaperclipMetadata) -> bool {
        metadata
            .relative_path(self.style())
            .is_some_and(|expected| expected == self.relative_path)
    }

    #[must_use]
    pub fn relative_path(&self) -> &Path {
        Path::new(&self.relative_path)
    }

    #[must_use]
    pub const fn attachment(&self) -> PaperclipAttachment {
        self.attachment
    }

    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    fn style(&self) -> &str {
        self.relative_path
            .rsplit_once('/')
            .and_then(|(parent, _)| parent.rsplit_once('/'))
            .map_or("", |(_, style)| style)
    }
}

#[must_use]
pub fn parse_paperclip_path(path: &str) -> Option<PaperclipPath> {
    if path.is_empty() || path.starts_with('/') || !valid_percent_encoding(path) {
        return None;
    }
    let components = path
        .split('/')
        .map(decode_component)
        .collect::<Option<Vec<_>>>()?;
    if components.len() < 7 {
        return None;
    }

    let cache_offset = usize::from(components.first().is_some_and(|value| value == "cache"));
    let attachment_path = format!(
        "{}/{}",
        components.get(cache_offset)?,
        components.get(cache_offset + 1)?
    );
    let attachment = attachment_for_path(&attachment_path)?;
    if cache_offset == 1 && !attachment.permits_cache() {
        return None;
    }
    let partition_start = cache_offset + 2;
    let style_index = components.len().checked_sub(2)?;
    if style_index <= partition_start {
        return None;
    }
    let partition = components.get(partition_start..style_index)?;
    if partition.len() < 3
        || partition
            .iter()
            .any(|part| part.len() != 3 || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let id = partition.concat().parse::<i64>().ok()?;
    if partitioned_id(id)?
        .split('/')
        .ne(partition.iter().map(String::as_str))
    {
        return None;
    }
    let style = components.get(style_index)?;
    let file_name = components.get(style_index + 1)?;
    if !style
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'@'))
        || !attachment_permits_style(attachment, style)
        || !safe_component(file_name)
    {
        return None;
    }
    let relative_path = components.join("/");
    Some(PaperclipPath {
        relative_path,
        attachment,
        id,
    })
}

#[must_use]
pub fn partitioned_id(id: i64) -> Option<String> {
    if id <= 0 {
        return None;
    }
    let digits = id.to_string();
    let width = if digits.len() <= 9 {
        9
    } else if digits.len() <= 12 {
        12
    } else if digits.len().is_multiple_of(3) {
        digits.len()
    } else {
        return None;
    };
    let padded = format!("{digits:0>width$}");
    padded
        .as_bytes()
        .chunks_exact(3)
        .map(std::str::from_utf8)
        .collect::<Result<Vec<_>, _>>()
        .ok()
        .map(|parts| parts.join("/"))
}

#[must_use]
pub fn encode_url_path(path: &str) -> String {
    path.split('/')
        .map(|component| utf8_percent_encode(component, URL_PATH_COMPONENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

#[must_use]
pub fn rails_blank(value: &str) -> bool {
    value.chars().all(char::is_whitespace)
}

/// Opens a regular file beneath `root` without following any symlink component.
///
/// # Errors
///
/// Returns an I/O error if the root or file cannot be opened securely, or if the target is not a
/// regular file.
pub fn open_paperclip_file(root: &Path, relative_path: &Path) -> std::io::Result<File> {
    PaperclipRoot::open(root)?.open_file(relative_path)
}

fn attachment_for_path(path: &str) -> Option<PaperclipAttachment> {
    [
        PaperclipAttachment::AccountAvatar,
        PaperclipAttachment::AccountHeader,
        PaperclipAttachment::MediaFile,
        PaperclipAttachment::MediaThumbnail,
        PaperclipAttachment::CustomEmojiImage,
        PaperclipAttachment::PreviewCardImage,
        PaperclipAttachment::PreviewCardProviderIcon,
        PaperclipAttachment::SiteUploadFile,
    ]
    .into_iter()
    .find(|attachment| attachment.path() == path)
}

fn attachment_permits_style(attachment: PaperclipAttachment, style: &str) -> bool {
    match attachment {
        PaperclipAttachment::AccountAvatar | PaperclipAttachment::AccountHeader => {
            matches!(style, "original" | "static")
        }
        PaperclipAttachment::MediaFile => matches!(style, "original" | "small"),
        PaperclipAttachment::MediaThumbnail | PaperclipAttachment::PreviewCardImage => {
            style == "original"
        }
        PaperclipAttachment::CustomEmojiImage | PaperclipAttachment::PreviewCardProviderIcon => {
            matches!(style, "original" | "static")
        }
        PaperclipAttachment::SiteUploadFile => {
            style == "original"
                || APP_ICON_STYLES.contains(&style)
                || FAVICON_STYLES.contains(&style)
                || THUMBNAIL_STYLES.contains(&style)
                || style == "mascot"
        }
    }
}

fn derivative_file_name(file_name: &str, extension: &str) -> Option<String> {
    let extension_start = file_name.rfind('.').filter(|index| *index > 0);
    let base = extension_start.map_or(file_name, |index| &file_name[..index]);
    (!base.is_empty()).then(|| format!("{base}.{extension}"))
}

fn decode_component(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let decoded = percent_decode_str(value).decode_utf8().ok()?.into_owned();
    safe_component(&decoded).then_some(decoded)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && !value.contains(['/', '\\', '\0'])
        && !value.chars().any(char::is_control)
}

fn valid_percent_encoding(value: &str) -> bool {
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%'
            && !matches!(
                (bytes.next(), bytes.next()),
                (Some(first), Some(second)) if first.is_ascii_hexdigit() && second.is_ascii_hexdigit()
            )
        {
            return false;
        }
    }
    true
}

fn safe_relative_path(path: &Path) -> bool {
    let mut components = path.components();
    let mut count = 0_usize;
    for component in &mut components {
        let Component::Normal(component) = component else {
            return false;
        };
        let Some(component) = component.to_str() else {
            return false;
        };
        if !safe_component(component) {
            return false;
        }
        count += 1;
    }
    count > 0
}

#[cfg(test)]
mod media_probe_tests {
    use super::*;

    fn video_probe(format_name: &str, width: Value, height: Value) -> Value {
        json!({
            "format": {
                "format_name": format_name,
                "duration": "1.0",
                "bit_rate": "1000",
            },
            "streams": [{
                "codec_type": "video",
                "codec_name": "vp8",
                "pix_fmt": "yuv420p",
                "width": width,
                "height": height,
                "avg_frame_rate": "30/1",
                "nb_read_frames": "30",
                "disposition": {"attached_pic": 0},
            }],
        })
    }

    fn assert_invalid(probe: &Value, kind: MediaKind, content_type: &str) {
        assert_eq!(
            inspect_probe(probe, kind, content_type).unwrap_err(),
            MediaAttachmentError::InvalidMedia,
        );
    }

    #[test]
    fn container_signatures_disambiguate_shared_demuxers() {
        let avif = include_bytes!("../tests/fixtures/media/600x400.avif");
        let heic = include_bytes!("../tests/fixtures/media/600x400.heic");
        let webm = include_bytes!("../tests/fixtures/media/attachment.webm");
        let mp4 = include_bytes!("../tests/fixtures/media/capability.mp4");
        let quicktime = include_bytes!("../tests/fixtures/media/capability.mov");
        let m4a = include_bytes!("../tests/fixtures/media/capability.m4a");

        assert!(media_container_matches("image/avif", avif));
        assert!(!media_container_matches("image/heic", avif));
        assert!(media_container_matches("image/heic", heic));
        assert!(!media_container_matches("video/mp4", heic));
        assert!(media_container_matches("video/mp4", mp4));
        assert!(!media_container_matches("video/quicktime", mp4));
        assert!(media_container_matches("video/quicktime", quicktime));
        assert!(media_container_matches("video/mp4", m4a));
        assert!(media_container_matches("audio/m4a", m4a));
        assert!(!media_container_matches("video/quicktime", m4a));
        assert!(media_container_matches("video/webm", webm));

        let matroska_header = b"\x1a\x45\xdf\xa3\x8b\x42\x82\x88matroska";
        assert!(!media_container_matches("video/webm", matroska_header));
        let deceptive_matroska_header =
            b"\x1a\x45\xdf\xa3\x94\xec\x87\x42\x82\x84webm\x42\x82\x88matroska";
        assert!(!media_container_matches(
            "video/webm",
            deceptive_matroska_header,
        ));
    }

    #[test]
    fn probe_rejects_declared_container_mismatch() {
        assert_invalid(
            &video_probe("matroska,webm", json!(640), json!(480)),
            MediaKind::Video,
            "video/mp4",
        );
        assert_invalid(
            &json!({
                "format": {"format_name": "mp3", "duration": "1.0"},
                "streams": [{"codec_type": "audio", "codec_name": "mp3"}],
            }),
            MediaKind::Audio,
            "audio/ogg",
        );
        assert_invalid(
            &video_probe("webm,matroska", json!(640), json!(480)),
            MediaKind::Video,
            "video/webm",
        );
    }

    #[test]
    fn probe_applies_the_video_specific_matrix_limit() {
        assert_invalid(
            &video_probe("matroska,webm", json!(4000), json!(3000)),
            MediaKind::Video,
            "video/webm",
        );
    }

    #[test]
    fn probe_applies_image_matrix_and_still_topology_limits() {
        let mut probe = json!({
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2"},
            "streams": [{
                "codec_type": "video",
                "codec_name": "av1",
                "width": 7000,
                "height": 5000,
                "nb_read_frames": "1",
                "disposition": {"attached_pic": 0},
            }],
        });
        assert_invalid(&probe, MediaKind::Image, "image/avif");

        probe["streams"][0]["width"] = json!(600);
        probe["streams"][0]["height"] = json!(400);
        let auxiliary = probe["streams"][0].clone();
        probe["streams"].as_array_mut().unwrap().push(auxiliary);
        assert!(inspect_probe(&probe, MediaKind::Image, "image/avif").is_ok());

        probe["streams"][1]["nb_read_frames"] = json!("2");
        assert_invalid(&probe, MediaKind::Image, "image/avif");
        probe["streams"][1]["nb_read_frames"] = json!("1");
        probe["streams"].as_array_mut().unwrap().push(json!({
            "codec_type": "audio",
            "codec_name": "aac",
        }));
        assert_invalid(&probe, MediaKind::Image, "image/avif");
    }

    #[test]
    fn probe_rejects_malformed_numbers_duration_rate_and_frame_count() {
        let mut probe = video_probe("matroska,webm", json!(640), json!(480));
        probe["streams"][0]["width"] = json!("wide");
        assert_invalid(&probe, MediaKind::Video, "video/webm");

        let mut probe = video_probe("matroska,webm", json!(640), json!(480));
        probe["format"]["duration"] = json!("NaN");
        assert_invalid(&probe, MediaKind::Video, "video/webm");

        let mut probe = video_probe("matroska,webm", json!(640), json!(480));
        probe["format"]["duration"] = json!("3600.001");
        assert_invalid(&probe, MediaKind::Video, "video/webm");

        let mut probe = video_probe("matroska,webm", json!(640), json!(480));
        probe["streams"][0]["avg_frame_rate"] = json!("121/1");
        assert_invalid(&probe, MediaKind::Video, "video/webm");

        let mut probe = video_probe("matroska,webm", json!(640), json!(480));
        probe["streams"][0]["nb_read_frames"] = json!("36001");
        assert_invalid(&probe, MediaKind::Video, "video/webm");

        let mut probe = video_probe("matroska,webm", json!(640), json!(480));
        probe["streams"][0]["avg_frame_rate"] = json!("30/nope");
        assert_invalid(&probe, MediaKind::Video, "video/webm");
    }

    #[test]
    fn normalized_video_requires_browser_codec_pixel_format_and_topology() {
        let mut probe = video_probe("mov,mp4,m4a,3gp,3g2,mj2", json!(640), json!(480));
        probe["streams"][0]["codec_name"] = json!("h264");
        assert!(inspect_normalized_video(&probe).is_ok());

        probe["streams"][0]["codec_name"] = json!("hevc");
        assert_eq!(
            inspect_normalized_video(&probe).unwrap_err(),
            MediaAttachmentError::InvalidMedia,
        );
        probe["streams"][0]["codec_name"] = json!("h264");
        probe["streams"][0]["pix_fmt"] = json!("yuv444p");
        assert!(inspect_normalized_video(&probe).is_err());
        probe["streams"][0]["pix_fmt"] = json!("yuv420p");
        probe["streams"].as_array_mut().unwrap().push(json!({
            "codec_type": "audio",
            "codec_name": "opus",
        }));
        assert!(inspect_normalized_video(&probe).is_err());
        probe["streams"][1]["codec_name"] = json!("aac");
        assert!(inspect_normalized_video(&probe).is_ok());
        probe["streams"].as_array_mut().unwrap().push(json!({
            "codec_type": "subtitle",
            "codec_name": "mov_text",
        }));
        assert!(inspect_normalized_video(&probe).is_err());
    }

    #[test]
    fn normalized_audio_requires_mp3_and_allows_only_attached_cover_video() {
        let mut probe = json!({
            "format": {"format_name": "mp3"},
            "streams": [{"codec_type": "audio", "codec_name": "mp3"}],
        });
        assert!(inspect_normalized_audio(&probe, Some(1.0)).is_ok());
        probe["streams"].as_array_mut().unwrap().push(json!({
            "codec_type": "video",
            "codec_name": "png",
            "width": 100,
            "height": 100,
            "nb_read_frames": "1",
            "disposition": {"attached_pic": 0},
        }));
        assert!(inspect_normalized_audio(&probe, Some(1.0)).is_err());
        probe["streams"][1]["disposition"]["attached_pic"] = json!(1);
        assert!(inspect_normalized_audio(&probe, Some(1.0)).is_ok());
        probe["streams"][0]["codec_name"] = json!("aac");
        assert!(inspect_normalized_audio(&probe, Some(1.0)).is_err());
    }

    #[test]
    fn ffmpeg_progress_duration_is_bounded_and_strictly_parsed() {
        assert_eq!(
            parse_progress_duration(b"out_time_us=1000000\nprogress=end\n").unwrap(),
            Some(1.0),
        );
        assert_eq!(
            parse_progress_duration(b"out_time_us=nope\n").unwrap_err(),
            MediaAttachmentError::InvalidMedia,
        );
        assert_eq!(
            parse_progress_duration(b"out_time_us=3600001000\n").unwrap_err(),
            MediaAttachmentError::InvalidMedia,
        );
    }
}

#[cfg(test)]
mod profile_output_tests {
    use super::*;
    #[test]
    fn profile_encoder_output_is_bounded_during_writes() {
        let mut output = BoundedImageBytes(vec![0; ACCOUNT_MEDIA_LIMIT - 1]);
        assert_eq!(output.write(&[1]).unwrap(), 1);
        assert!(output.write(&[2]).is_err());
        assert_eq!(output.0.len(), ACCOUNT_MEDIA_LIMIT);
    }
}
