//! Media upload, update, show and delete endpoints.

use super::*;

pub(super) async fn media_create_v1(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    media_create(state, rack, headers, false).await
}

pub(super) async fn media_create_v2(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    media_create(state, rack, headers, true).await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn media_create(
    state: WebState,
    rack: RackParameters,
    headers: HeaderMap,
    rich: bool,
) -> Response<Body> {
    let rate_limit_user_id = match optional_authenticated_user_id(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if let Some(user_id) = rate_limit_user_id
        && let Err(limited) = state
            .media_upload_limiter
            .check_shared(state.shared_rate_limiter.as_ref(), user_id)
            .await
    {
        return rate_limited_response(limited);
    }
    let authenticated = match required_write_viewer(&state, &headers, WRITE_MEDIA).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Some(RackValue::Upload(upload)) = rack.get("file") else {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "File type of uploaded media could not be verified",
        );
    };
    let description = match media_description_parameter(&rack, "description") {
        Ok(AccountProfileValue::Unchanged | AccountProfileValue::Null) => None,
        Ok(AccountProfileValue::Value(value)) => Some(value),
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, error),
    };
    let focus = match media_focus_parameter(&rack, "focus") {
        Ok(focus) => focus,
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, error),
    };
    if rich
        && crate::media::media_format(&upload.content_type)
            .is_some_and(|format| format.external_processing)
    {
        return media_create_rich(
            &state,
            &authenticated,
            upload,
            MediaAttachmentUpdate {
                description: description
                    .map_or(AccountProfileValue::Null, AccountProfileValue::Value),
                focus,
            },
        )
        .await;
    }
    let prepared = match crate::paperclip::prepare_media_attachment_async(
        owner.account_id(),
        upload.file_name.clone(),
        upload.content_type.clone(),
        upload.bytes.clone(),
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => return media_upload_error(error),
    };
    let create = MediaAttachmentCreate {
        media_type: prepared.media_kind.database_type(),
        file_name: prepared.file_name.clone(),
        content_type: prepared.content_type.clone(),
        file_size: prepared.file_size,
        file_meta: prepared.file_meta.clone(),
        blurhash: prepared.blurhash.clone(),
        description,
        focus,
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let account_id = owner.account_id();
    match writer
        .with_account_lock(account_id, || async {
            let id = writer
                .stage_media_attachment_locked(&authenticated, &create)
                .await?;
            let metadata = media_metadata_from_prepared(id, &prepared);
            if let Err(error) = write_prepared_media(&state.media_root, &metadata, &prepared) {
                remove_expected_media(&state.media_root, &metadata);
                return Err(WriteError::Filesystem(error));
            }
            // A failed commit has an ambiguous outcome. Retain the files so either the
            // published row is complete or its durable rollback intent removes them.
            writer
                .publish_media_attachment_locked(&authenticated, id, &create)
                .await?;
            let response = media_response_for_id(&state, account_id, id).await;
            if !response.status().is_success() {
                let _ = cleanup_media_after_response_failure(
                    &state.media_root,
                    writer,
                    &authenticated,
                    id,
                )
                .await;
            }
            Ok(response)
        })
        .await
    {
        Ok(response) => response,
        Err(error) => media_write_error(&error),
    }
}

// Modern stills are intentionally asynchronous too: the bundled composer polls
// 206 until ready, or terminates on retained 422. No filename exists before publish.
pub(super) async fn media_create_rich(
    state: &WebState,
    authenticated: &AuthenticatedBearer,
    upload: &UploadedFile,
    update: MediaAttachmentUpdate,
) -> Response<Body> {
    use crate::mastodon::local_uploads::{RawInput, raw_path};
    use sha2::{Digest, Sha256};
    let Some(format) = crate::media::media_format(&upload.content_type) else {
        return media_upload_error(crate::paperclip::MediaAttachmentError::UnsupportedContentType);
    };
    if upload.bytes.is_empty() || upload.bytes.len() >= format.input_size_limit {
        return media_upload_error(crate::paperclip::MediaAttachmentError::TooLarge);
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let account = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let bytes = upload.bytes.clone();
    let hash: [u8; 32] =
        match tokio::task::spawn_blocking(move || Sha256::digest(bytes).into()).await {
            Ok(hash) => hash,
            Err(_) => return internal_error(),
        };
    match writer
        .with_account_lock(account, || async {
            let id = writer
                .stage_local_upload_locked(
                    authenticated,
                    &RawInput {
                        mime: &upload.content_type,
                        size: i64::try_from(upload.bytes.len()).expect("bounded upload length"),
                        sha256: &hash,
                    },
                    &update,
                )
                .await?;
            // Stage commit precedes all writes. Synchronous confined write/fsync remains
            // inside the account lock, including cancellation; no detached write may
            // outlive ownership. Every ambiguous/error outcome retains its manifest.
            state
                .media_root
                .private_upload_root()?
                .write_file(FsPath::new(&raw_path(id)), &upload.bytes)?;
            writer.accept_local_upload_locked(authenticated, id).await?;
            let mut response = media_response_for_id(state, account, id.media_id).await;
            if response.status() == StatusCode::PARTIAL_CONTENT {
                *response.status_mut() = StatusCode::ACCEPTED;
            }
            Ok(response)
        })
        .await
    {
        Ok(response) => response,
        Err(error) => media_write_error(&error),
    }
}

pub(super) async fn media_show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, WRITE_MEDIA).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((_, id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    media_response_for_id(&state, owner, id).await
}

pub(super) async fn media_update(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_MEDIA).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Some((_, id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let current = match state
        .repository
        .media_attachment(owner.account_id(), id)
        .await
    {
        Ok(Some(media)) => media,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if current
        .processing
        .is_some_and(|processing| processing.0 == 3)
    {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Error processing thumbnail for uploaded media",
        );
    }
    let update = match media_attachment_update(&rack) {
        Ok(update) => update,
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, error),
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .update_media_attachment(&authenticated, id, &update)
        .await
    {
        return media_write_error(&error);
    }
    media_response_for_id(&state, owner.account_id(), id).await
}

pub(super) async fn media_delete(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_MEDIA).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let account_id = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    if let Err(error) = writer
        .with_account_lock(account_id, || async {
            let media = writer
                .delete_media_attachment_locked(&authenticated, id)
                .await?;
            remove_media_files(&state.media_root, &media);
            Ok(())
        })
        .await
    {
        return media_write_error(&error);
    }
    empty_json_response()
}

pub(super) async fn media_response_for_id(
    state: &WebState,
    account_id: i64,
    id: i64,
) -> Response<Body> {
    let media = match state.repository.media_attachment(account_id, id).await {
        Ok(Some(media))
            if media.file_file_name.is_some()
                || (media.remote_url.is_empty()
                    && media
                        .processing
                        .is_some_and(|state| matches!(state.0, 1 | 3))) =>
        {
            media
        }
        Ok(Some(_) | None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if media.processing.is_some_and(|processing| processing.0 == 3) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Error processing thumbnail for uploaded media",
        );
    }
    let status = if media.processing.is_some_and(|processing| processing.0 != 2) {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let projection = media_projection(&media, media.description.clone());
    match serde_json::to_vec(&state.serializer().media_attachment(&projection)) {
        Ok(body) => json_response(status, body),
        Err(_) => internal_error(),
    }
}

pub(super) fn media_attachment_update(
    rack: &RackParameters,
) -> Result<MediaAttachmentUpdate, &'static str> {
    Ok(MediaAttachmentUpdate {
        description: media_description_parameter(rack, "description")?,
        focus: media_focus_parameter(rack, "focus")?,
    })
}

pub(super) fn media_description_parameter(
    rack: &RackParameters,
    name: &str,
) -> Result<AccountProfileValue<String>, &'static str> {
    media_description_value(rack.get(name))
}

pub(super) fn media_description_value(
    value: Option<&RackValue>,
) -> Result<AccountProfileValue<String>, &'static str> {
    match value {
        None => Ok(AccountProfileValue::Unchanged),
        Some(RackValue::Null) => Ok(AccountProfileValue::Null),
        Some(value) => profile_scalar_string(value)
            .map(AccountProfileValue::Value)
            .map_err(|()| "Invalid media description"),
    }
}

pub(super) fn media_focus_parameter(
    rack: &RackParameters,
    name: &str,
) -> Result<AccountProfileValue<MediaFocus>, &'static str> {
    media_focus_value(rack.get(name))
}

pub(super) fn media_focus_value(
    value: Option<&RackValue>,
) -> Result<AccountProfileValue<MediaFocus>, &'static str> {
    let Some(value) = value else {
        return Ok(AccountProfileValue::Unchanged);
    };
    if matches!(value, RackValue::Null)
        || matches!(value, RackValue::Scalar(value) if value.trim().is_empty())
    {
        return Ok(AccountProfileValue::Unchanged);
    }
    let values = match value {
        RackValue::Scalar(value) => value.split(',').map(str::to_owned).collect::<Vec<_>>(),
        RackValue::Array(values) if values.len() == 2 => values
            .iter()
            .map(profile_scalar_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|()| "Invalid media focus")?,
        RackValue::Object(values) => ["x", "y"]
            .into_iter()
            .map(|key| values.get(key).ok_or(()).and_then(profile_scalar_string))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|()| "Invalid media focus")?,
        _ => return Err("Invalid media focus"),
    };
    if values.len() != 2 {
        return Err("Invalid media focus");
    }
    let x = values[0]
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or("Invalid media focus")?;
    let y = values[1]
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or("Invalid media focus")?;
    Ok(AccountProfileValue::Value(MediaFocus { x, y }))
}

pub(super) fn media_metadata_from_prepared(
    id: i64,
    prepared: &PreparedMediaAttachment,
) -> PaperclipMetadata {
    PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id,
        remote: false,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    }
}

pub(super) fn media_metadata_from_record(
    media: &crate::mastodon::MediaAttachment,
) -> Option<PaperclipMetadata> {
    Some(PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: media.id,
        remote: !crate::paperclip::rails_blank(&media.remote_url),
        storage_schema_version: media.file_storage_schema_version,
        file_name: media.file_file_name.clone()?,
        content_type: media.file_content_type.clone(),
        variant: None,
    })
}

pub(super) fn media_thumbnail_metadata_from_record(
    media: &crate::mastodon::MediaAttachment,
) -> Option<PaperclipMetadata> {
    Some(PaperclipMetadata {
        attachment: PaperclipAttachment::MediaThumbnail,
        id: media.id,
        remote: !crate::paperclip::rails_blank(&media.remote_url),
        storage_schema_version: media.thumbnail_storage_schema_version,
        file_name: media.thumbnail_file_name.clone()?,
        content_type: media.thumbnail_content_type.clone(),
        variant: None,
    })
}

pub(super) fn remove_media_files(root: &PaperclipRoot, media: &crate::mastodon::MediaAttachment) {
    if let Some(metadata) = media_metadata_from_record(media) {
        for style in ["original", "small"] {
            if let Some(path) = metadata.relative_path(style) {
                let _ = root.remove_file(FsPath::new(&path));
            }
        }
    }
    if let Some(metadata) = media_thumbnail_metadata_from_record(media)
        && let Some(path) = metadata.relative_path("original")
    {
        let _ = root.remove_file(FsPath::new(&path));
    }
}

pub(super) async fn cleanup_media_after_response_failure(
    root: &PaperclipRoot,
    writer: &WriteRepository,
    authenticated: &AuthenticatedBearer,
    id: i64,
) -> Result<(), WriteError> {
    let media = writer
        .delete_media_attachment_locked(authenticated, id)
        .await?;
    remove_media_files(root, &media);
    Ok(())
}

#[cfg(feature = "test-support")]
/// Exercises the same ordered cleanup used after media response serialization fails.
///
/// # Errors
///
/// Returns the metadata transaction error without unlinking any published files.
pub async fn cleanup_media_after_response_failure_for_test(
    root: &PaperclipRoot,
    writer: &WriteRepository,
    authenticated: &AuthenticatedBearer,
    id: i64,
) -> Result<(), WriteError> {
    let account_id = authenticated
        .require_user()
        .map_err(|_| WriteError::Unauthorized)?
        .account_id();
    writer
        .with_account_lock(account_id, || async {
            cleanup_media_after_response_failure(root, writer, authenticated, id).await
        })
        .await
}

pub(super) fn remove_expected_media(root: &PaperclipRoot, metadata: &PaperclipMetadata) {
    for style in ["original", "small"] {
        if let Some(path) = metadata.relative_path(style) {
            let _ = root.remove_file(FsPath::new(&path));
        }
    }
}

pub(super) fn media_upload_error(error: crate::paperclip::MediaAttachmentError) -> Response<Body> {
    let (status, message) = match error {
        crate::paperclip::MediaAttachmentError::TooLarge => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "File size of uploaded media is too large",
        ),
        crate::paperclip::MediaAttachmentError::ProcessingTimedOut => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Uploaded media took too long to process",
        ),
        crate::paperclip::MediaAttachmentError::ProcessingUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Media processing is temporarily unavailable",
        ),
        crate::paperclip::MediaAttachmentError::UnsupportedContentType
        | crate::paperclip::MediaAttachmentError::InvalidImage
        | crate::paperclip::MediaAttachmentError::InvalidMedia
        | crate::paperclip::MediaAttachmentError::SizeOverflow => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "File type of uploaded media could not be verified",
        ),
    };
    error_response(status, message)
}

pub(super) fn media_write_error(error: &WriteError) -> Response<Body> {
    match error {
        WriteError::Unauthorized => error_response(StatusCode::UNAUTHORIZED, "Unauthorized"),
        WriteError::Forbidden => {
            error_response(StatusCode::FORBIDDEN, "This action is not allowed")
        }
        WriteError::NotFound => record_not_found(),
        WriteError::Validation(message) => {
            error_response(StatusCode::UNPROCESSABLE_ENTITY, message)
        }
        WriteError::InvalidInput(message) => error_response(StatusCode::BAD_REQUEST, message),
        WriteError::Conflict => error_response(
            StatusCode::CONFLICT,
            "Conflict during update, please try again",
        ),
        WriteError::RateLimited
        | WriteError::Sqlx(_)
        | WriteError::Job(_)
        | WriteError::Filesystem(_) => internal_error(),
    }
}
