use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path};
use std::sync::Arc;
#[cfg(feature = "test-support")]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

const IMAGE_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/heic",
    "image/heif",
    "image/webp",
    "image/avif",
];
const CONVERTED_IMAGE_MIME_TYPES: &[&str] = &["image/heic", "image/heif", "image/avif"];
const VIDEO_MIME_TYPES: &[&str] = &["video/webm", "video/mp4", "video/quicktime", "video/ogg"];
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
    pub file_name: String,
    pub content_type: String,
    pub file_size: i32,
    pub original_bytes: Vec<u8>,
    pub small_file_name: String,
    pub small_content_type: String,
    pub small_bytes: Vec<u8>,
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
    SizeOverflow,
}

impl std::fmt::Display for MediaAttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedContentType => "unsupported media image content type",
            Self::TooLarge => "media image is too large",
            Self::InvalidImage => "media image is invalid",
            Self::SizeOverflow => "media image size is too large",
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
    if !matches!(
        content_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    ) {
        return Err(MediaAttachmentError::UnsupportedContentType);
    }
    if bytes.len() >= 16 * 1024 * 1024 {
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
        file_name,
        content_type: content_type.to_owned(),
        file_size,
        original_bytes,
        small_file_name,
        small_content_type: if format == ImageFormat::Gif {
            "image/png".to_owned()
        } else {
            content_type.to_owned()
        },
        small_bytes,
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
                let content_type = self.content_type.as_deref()?;
                if content_type == "image/gif" || VIDEO_MIME_TYPES.contains(&content_type) {
                    png()
                } else if CONVERTED_IMAGE_MIME_TYPES.contains(&content_type) {
                    derivative_file_name(&self.file_name, "jpeg")
                } else if IMAGE_MIME_TYPES.contains(&content_type) {
                    Some(self.file_name.clone())
                } else {
                    None
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
    let small = metadata.relative_path("small").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid media path")
    })?;
    write_prepared_file(root, Path::new(&original), &prepared.original_bytes)?;
    if let Err(error) = write_prepared_file(root, Path::new(&small), &prepared.small_bytes) {
        let _ = root.remove_file(Path::new(&original));
        let _ = root.remove_file(Path::new(&small));
        return Err(error);
    }
    Ok(vec![original, small])
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
