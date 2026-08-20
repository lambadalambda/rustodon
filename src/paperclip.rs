use std::fs::File;
use std::path::{Component, Path};
use std::sync::Arc;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use rustix::fd::OwnedFd;
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags, fstat, open, openat2};

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
        })
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
