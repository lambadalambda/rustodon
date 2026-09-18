pub const IMAGE_SIZE_LIMIT: usize = 16 * 1024 * 1024;
pub const AUDIO_VIDEO_SIZE_LIMIT: usize = 99 * 1024 * 1024;

pub const ALL_MEDIA_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/heic",
    "image/heif",
    "image/webp",
    "image/avif",
    "video/webm",
    "video/mp4",
    "video/quicktime",
    "video/ogg",
    "audio/wave",
    "audio/wav",
    "audio/x-wav",
    "audio/x-pn-wave",
    "audio/vnd.wave",
    "audio/ogg",
    "audio/vorbis",
    "audio/mpeg",
    "audio/mp3",
    "audio/webm",
    "audio/flac",
    "audio/aac",
    "audio/m4a",
    "audio/x-m4a",
    "audio/mp4",
    "audio/3gpp",
    "video/x-ms-asf",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Image,
    Gif,
    Video,
    Audio,
}

impl MediaKind {
    #[must_use]
    pub const fn database_type(self) -> i32 {
        match self {
            Self::Image => 0,
            Self::Gif => 1,
            Self::Video => 2,
            Self::Audio => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaFormat {
    pub kind: MediaKind,
    pub input_size_limit: usize,
    pub external_processing: bool,
    pub output_content_type: &'static str,
    pub preview_content_type: Option<&'static str>,
}

const fn format(
    kind: MediaKind,
    input_size_limit: usize,
    external_processing: bool,
    output_content_type: &'static str,
    preview_content_type: Option<&'static str>,
) -> MediaFormat {
    MediaFormat {
        kind,
        input_size_limit,
        external_processing,
        output_content_type,
        preview_content_type,
    }
}

#[must_use]
pub const fn media_format(content_type: &str) -> Option<MediaFormat> {
    match content_type.as_bytes() {
        b"image/jpeg" => Some(format(
            MediaKind::Image,
            IMAGE_SIZE_LIMIT,
            false,
            "image/jpeg",
            Some("image/jpeg"),
        )),
        b"image/png" => Some(format(
            MediaKind::Image,
            IMAGE_SIZE_LIMIT,
            false,
            "image/png",
            Some("image/png"),
        )),
        b"image/gif" => Some(format(
            MediaKind::Gif,
            IMAGE_SIZE_LIMIT,
            false,
            "image/gif",
            Some("image/png"),
        )),
        b"image/webp" => Some(format(
            MediaKind::Image,
            IMAGE_SIZE_LIMIT,
            false,
            "image/webp",
            Some("image/webp"),
        )),
        b"image/heic" | b"image/heif" | b"image/avif" => Some(format(
            MediaKind::Image,
            IMAGE_SIZE_LIMIT,
            true,
            "image/jpeg",
            Some("image/jpeg"),
        )),
        b"video/webm" | b"video/mp4" | b"video/quicktime" | b"video/ogg" | b"video/x-ms-asf" => {
            Some(format(
                MediaKind::Video,
                AUDIO_VIDEO_SIZE_LIMIT,
                true,
                "video/mp4",
                Some("image/png"),
            ))
        }
        b"audio/wave" | b"audio/wav" | b"audio/x-wav" | b"audio/x-pn-wave" | b"audio/vnd.wave"
        | b"audio/ogg" | b"audio/vorbis" | b"audio/mpeg" | b"audio/mp3" | b"audio/webm"
        | b"audio/flac" | b"audio/aac" | b"audio/m4a" | b"audio/x-m4a" | b"audio/mp4"
        | b"audio/3gpp" => Some(format(
            MediaKind::Audio,
            AUDIO_VIDEO_SIZE_LIMIT,
            true,
            "audio/mpeg",
            None,
        )),
        _ => None,
    }
}
