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

/// Validated MIME agreement for one remote attachment, not a byte-level probe.
/// Missing advertisement is an explicit compatibility mode: the response must
/// still name a supported format and is bounded by that format before streaming.
#[derive(Clone, Copy, Debug)]
pub struct RemoteMediaPolicy {
    advertised: Option<&'static str>,
}

impl RemoteMediaPolicy {
    /// Returns `None` for an explicitly advertised unsupported MIME type.
    #[must_use]
    pub fn new(advertised: Option<&str>) -> Option<Self> {
        Some(Self {
            advertised: match advertised {
                Some(value) => Some(normalized_media_mime(value)?),
                None => None,
            },
        })
    }

    /// MIME parameters and casing do not affect agreement. Different MIME
    /// essences do, even within the same family; only probing can validate bytes.
    #[must_use]
    pub fn response_format(self, fetched: &str) -> Option<MediaFormat> {
        let fetched = normalized_media_mime(fetched)?;
        if self
            .advertised
            .is_some_and(|advertised| advertised != fetched)
        {
            return None;
        }
        media_format(fetched)
    }
}

fn normalized_media_mime(value: &str) -> Option<&'static str> {
    let essence = value.split(';').next()?.trim();
    ALL_MEDIA_MIME_TYPES
        .iter()
        .copied()
        .find(|supported| supported.eq_ignore_ascii_case(essence))
}
