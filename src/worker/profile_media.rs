//! Bounded downloads for remote account avatars and headers, never REST hotlinks.
use super::*;
use crate::mastodon::profile_media::{
    ProfileImageJob, ProfileImageSlot, cleanup_job, image_paths, image_state,
};
use crate::paperclip::{prepare_account_media, write_prepared_account_media};

#[allow(clippy::too_many_lines)]
pub(super) async fn fetch_profile_image(
    pool: PgPool,
    queue: Queue,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    root: PaperclipRoot,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let job: ProfileImageJob = serde_json::from_value(arguments.clone())
        .map_err(|_| HandlerFailure::permanent("profile image job is invalid"))?;
    let mut transaction = pool.begin().await.map_err(database_failure)?;
    let current = image_state(&mut transaction, job.account_id, job.slot)
        .await
        .map_err(database_failure)?;
    transaction.rollback().await.map_err(database_failure)?;
    let Some(current) = current.filter(|state| state.matches(&job)) else {
        return Ok(());
    };
    let url = Url::parse(&job.remote_url)
        .map_err(|_| HandlerFailure::permanent("profile image URL is invalid"))?;
    let image_domain = canonical_remote_domain_from_url(&url)
        .map_err(|_| HandlerFailure::permanent("profile image domain is invalid"))?;
    let repository = Repository::from_pool(pool.clone());
    for domain in [job.domain.as_str(), image_domain.as_str()] {
        if !repository
            .remote_media_allowed(domain, config.limited_federation)
            .await
            .map_err(database_failure)?
        {
            return Ok(());
        }
    }
    // A repeated discovery/refresh is a cache repair, not a forced download.
    // GIF static derivatives have no SQL size/hash: conservatively reprepare them
    // to repair nonempty truncated derivatives as well as missing originals.
    if current.content_type.as_deref() != Some("image/gif")
        && current.file_size.is_some_and(|size| size > 0)
        && current.content_type.is_some()
        && let Some(metadata) = current.metadata(job.account_id, job.slot)
        && !image_paths(&metadata).is_empty()
        && image_paths(&metadata).iter().all(|path| {
            root.open_file(Path::new(path))
                .and_then(|file| file.metadata())
                .is_ok_and(|file| {
                    if Some(path) == metadata.relative_path("original").as_ref() {
                        current.file_size.and_then(|size| u64::try_from(size).ok())
                            == Some(file.len())
                    } else {
                        file.len() > 0
                    }
                })
        })
    {
        return Ok(());
    }
    let fetcher = fetcher.with_limits(RemoteFetchLimits {
        max_response_bytes: 8 * 1024 * 1024 - 1,
        ..RemoteFetchLimits::default()
    });
    // Extension and advertised HTTP type are not trusted. The bounded decoder below
    // determines the image format, including CDN URLs ending in .blob.
    #[cfg(feature = "test-support")]
    let fetcher = fetcher.with_test_endpoint(config.remote_media_endpoint);
    let result = fetcher
        .get_with_policy(url, |url| {
            let repository = Repository::from_pool(pool.clone());
            async move {
                let domain = canonical_remote_domain_from_url(&url)?;
                if repository
                    .remote_media_allowed(&domain, config.limited_federation)
                    .await
                    .map_err(|_| RemoteFetchError::Request)?
                {
                    Ok(())
                } else {
                    Err(RemoteFetchError::PolicyDenied)
                }
            }
        })
        .await;
    let (response, visited) = match result {
        Err(RemoteFetchError::PolicyDenied) => return Ok(()),
        other => other.map_err(|error| remote_media_fetch_failure(&error))?,
    };
    let mut domains = vec![job.domain.clone()];
    domains.extend(
        visited
            .iter()
            .map(canonical_remote_domain_from_url)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| HandlerFailure::permanent("profile image domain is invalid"))?,
    );
    let content_type = image::guess_format(&response.body)
        .map_err(|_| HandlerFailure::permanent("profile image has no supported decoded format"))?
        .to_mime_type();
    // Keep the worker's Media permit while bounded CPU processing runs off the
    // async runtime, so ingress and lease renewal remain responsive.
    let slot = job.slot;
    let id = job.account_id;
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_account_media(
            slot.attachment(),
            id,
            "profile",
            content_type,
            &response.body,
        )
    })
    .await
    .map_err(|_| HandlerFailure::retry("profile image processor failed"))?
    .map_err(|_| {
        HandlerFailure::permanent("profile image is invalid or exceeds processing limits")
    })?;
    let metadata = PaperclipMetadata {
        attachment: job.slot.attachment(),
        id: job.account_id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let writer = WriteRepository::from_pool(pool.clone());
    writer
        .with_remote_domains_locks(
            &domains.iter().map(String::as_str).collect::<Vec<_>>(),
            || async {
                let mut transaction = pool.begin().await?;
                let current = image_state(&mut transaction, job.account_id, job.slot).await?;
                let Some(current) = current.filter(|state| state.matches(&job)) else {
                    return Ok(());
                };
                for domain in &domains {
                    if !writer
                        .remote_media_allowed_in_transaction(
                            &mut transaction,
                            domain,
                            config.limited_federation,
                        )
                        .await?
                    {
                        return Ok(());
                    }
                }
                // Persist reconciliation before touching files. It runs under the same account
                // row lock and retains whatever paths the committed row names, even if COMMIT's
                // result is ambiguous or the process dies between writing files and SQL.
                let mut paths = image_paths(&metadata);
                if let Some(old) = current.metadata(job.account_id, job.slot) {
                    paths.extend(image_paths(&old));
                }
                queue
                    .enqueue(&cleanup_job(job.account_id, job.slot, &paths))
                    .await?;
                write_prepared_account_media(&root, &metadata, &prepared)
                    .map_err(WriteError::Filesystem)?;
                let column = job.slot.column();
                sqlx::query(&format!(
                    "UPDATE accounts SET {column}_file_name = $2,
            {column}_content_type = $3, {column}_file_size = $4,
            {column}_storage_schema_version = 1, updated_at = clock_timestamp() WHERE id = $1"
                ))
                .bind(job.account_id)
                .bind(&prepared.file_name)
                .bind(&prepared.content_type)
                .bind(prepared.file_size)
                .execute(&mut *transaction)
                .await?;
                #[cfg(feature = "test-support")]
                if root.take_commit_before_fault() {
                    return Err(WriteError::Validation("profile image commit failed"));
                }
                transaction.commit().await?;
                #[cfg(feature = "test-support")]
                if root.take_commit_after_fault() {
                    return Err(WriteError::Validation("profile image commit failed"));
                }
                Ok(())
            },
        )
        .await
        .map_err(|_| HandlerFailure::retry("profile image cache installation failed"))
}

pub(super) async fn cleanup_profile_images(
    pool: PgPool,
    root: PaperclipRoot,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    #[derive(serde::Deserialize)]
    struct Cleanup {
        account_id: i64,
        slot: ProfileImageSlot,
        paths: Vec<String>,
    }
    let job: Cleanup = serde_json::from_value(arguments.clone())
        .map_err(|_| HandlerFailure::permanent("profile image cleanup job is invalid"))?;
    if job.paths.is_empty()
        || job.paths.iter().any(|path| {
            !safe_cleanup_path(path)
                || !parse_paperclip_path(path).is_some_and(|parsed| {
                    parsed.id() == job.account_id && parsed.attachment() == job.slot.attachment()
                })
        })
    {
        return Err(HandlerFailure::permanent(
            "profile image cleanup path is invalid",
        ));
    }
    let mut transaction = pool.begin().await.map_err(database_failure)?;
    let current = image_state(&mut transaction, job.account_id, job.slot)
        .await
        .map_err(database_failure)?;
    let current_paths = current
        .and_then(|state| state.metadata(job.account_id, job.slot))
        .map_or_else(Vec::new, |metadata| image_paths(&metadata));
    for path in job
        .paths
        .iter()
        .filter(|path| !current_paths.contains(path))
    {
        root.remove_file(Path::new(path))
            .map_err(|_| HandlerFailure::retry("profile image cleanup failed"))?;
    }
    transaction.commit().await.map_err(database_failure)?;
    Ok(())
}

fn database_failure(_: sqlx::Error) -> HandlerFailure {
    HandlerFailure::retry("profile image database operation failed")
}
