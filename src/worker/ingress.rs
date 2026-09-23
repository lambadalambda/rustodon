//! Inbound `ActivityPub` processing: inbox actors, relationships, notes and quotes.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

pub(super) async fn schedule_remote_follow_reject(
    pool: &PgPool,
    repository: &Repository,
    config: &ActivityPubDeliveryConfig,
    source_account_id: i64,
    target_account_id: i64,
    follow_uri: &str,
) -> Result<(), HandlerFailure> {
    let source_account = repository
        .account(source_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow source lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Follow source account is missing"))?;
    let target_account = repository
        .account(target_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow target lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Follow target account is missing"))?;
    if source_account.domain.is_none() || target_account.domain.is_some() {
        return Err(HandlerFailure::permanent(
            "remote Follow rejection accounts are not local/remote",
        ));
    }
    let remote_domain = source_account
        .domain
        .clone()
        .ok_or_else(|| HandlerFailure::permanent("remote Follow source has no domain"))?;
    let inbox_url = if source_account.inbox_url.is_empty() {
        source_account.shared_inbox_url.clone()
    } else {
        source_account.inbox_url.clone()
    };
    if inbox_url.is_empty() {
        return Err(HandlerFailure::permanent(
            "remote Follow source has no inbox",
        ));
    }
    let target_uri = activitypub::actor_url(&config.origin, &target_account);
    let body = activitypub::reject_with_uris(
        &target_uri,
        None,
        follow_uri,
        &activitypub::actor_url(&config.origin, &source_account),
    );
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": target_account_id,
            "inbox_url": inbox_url,
            "remote_domain": remote_domain,
            "body": body
        }),
    )
    .logical_key(activitypub::reject_delivery_logical_key(
        target_account_id,
        follow_uri,
        &inbox_url,
    ));
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow rejection outbox transaction failed"))?;
    record_outbox_once_in(&mut transaction, &delivery)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow rejection outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow rejection outbox commit failed"))?;
    Ok(())
}

pub(super) async fn resolve_inbox_actor(
    repository: &Repository,
    pool: &PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    job: &InboxJob,
    actor_uri: &str,
) -> Result<i64, HandlerFailure> {
    let allowed = repository
        .remote_domain_allowed(&job.remote_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote inbox domain policy lookup failed"))?;
    if !allowed {
        return Err(HandlerFailure::permanent(
            "remote inbox domain is not allowed",
        ));
    }
    if inbox_actor_domain(actor_uri)
        .is_none_or(|domain| !domain.eq_ignore_ascii_case(&job.remote_domain))
    {
        return Err(HandlerFailure::permanent(
            "remote inbox actor domain does not match the verified signer",
        ));
    }

    if let Some(key) = repository
        .activitypub_signature_key(&job.signature_key_id, config.origin.as_str())
        .await
        .map_err(|_| HandlerFailure::retry("remote inbox signature key lookup failed"))?
    {
        let account = repository
            .account(key.account_id)
            .await
            .map_err(|_| HandlerFailure::retry("remote inbox actor lookup failed"))?
            .ok_or_else(|| HandlerFailure::permanent("remote inbox signer account is missing"))?;
        if account
            .domain
            .as_deref()
            .is_none_or(|domain| !domain.eq_ignore_ascii_case(&job.remote_domain))
            || account.uri != actor_uri
        {
            return Err(HandlerFailure::permanent(
                "remote inbox actor does not match the verified signer",
            ));
        }
        return Ok(account.id);
    }

    let instance = repository
        .account(-99)
        .await
        .map_err(|_| HandlerFailure::retry("instance actor lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("instance actor is missing"))?;
    let private_key = instance
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("instance actor has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &instance)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    #[cfg(feature = "test-support")]
    let fetcher = &fetcher
        .clone()
        .with_test_endpoint(config.remote_fetch_endpoint);
    let resolver = RemoteAccountResolver::new(fetcher.clone());
    let resolution = resolver
        .resolve_key(&job.signature_key_id, Some(&signer))
        .await
        .map_err(|error| inbox_remote_failure(&error))?;
    if !resolution.domain.eq_ignore_ascii_case(&job.remote_domain)
        || resolution.actor.id.as_str() != actor_uri
    {
        return Err(HandlerFailure::permanent(
            "resolved remote actor does not match the verified signer",
        ));
    }
    WriteRepository::from_pool(pool.clone())
        .upsert_remote_actor(
            &resolution.actor.username,
            &resolution.domain,
            config.limited_federation,
            &resolution.actor,
        )
        .await
        .map_err(|_| HandlerFailure::retry("remote inbox actor persistence failed"))
}

pub(super) fn remote_poll_vote_allows_note_fallback(outcome: RemotePollVoteOutcome) -> bool {
    outcome == RemotePollVoteOutcome::NotPollVote
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn fetch_and_import_quote_authorization(
    pool: &PgPool,
    repository: &Repository,
    writer: &WriteRepository,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    source_account_id: i64,
    delivery_target_account_id: Option<i64>,
    object: &Value,
) -> Result<Option<(RemoteQuoteFetchReferences, Option<Value>)>, HandlerFailure> {
    let Some(references) = remote_note_quote_fetch_references(object) else {
        return Ok(None);
    };
    let existing_target_local = writer
        .remote_quote_target_is_local(&references.target_uri, config.origin.as_str())
        .await
        .map_err(|error| remote_note_write_failure(&error, "quoted status lookup failed"))?;
    if existing_target_local == Some(true) {
        return Ok(Some((references, None)));
    }
    let target_url = Url::parse(&references.target_uri)
        .map_err(|_| HandlerFailure::permanent("quoted status URL is invalid"))?;
    let target_domain = canonical_remote_domain_from_url(&target_url)
        .map_err(|_| HandlerFailure::permanent("quoted status domain is invalid"))?;
    if !repository
        .remote_domain_allowed(&target_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("quoted status domain policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "quoted status domain is not allowed",
        ));
    }
    let quote_to = remote_announce_audience(object, "to")?;
    let quote_cc = remote_announce_audience(object, "cc")?;
    let signer_account = resolve_note_fetch_signer(
        pool,
        source_account_id,
        delivery_target_account_id,
        &quote_to,
        &quote_cc,
        &config.origin,
    )
    .await?;
    let private_key = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("quote fetch signer has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let authorization_document = if let Some(approval_uri) = references.approval_uri.as_deref() {
        let approval_url = Url::parse(approval_uri)
            .map_err(|_| HandlerFailure::permanent("quote authorization URL is invalid"))?;
        let approval_domain = canonical_remote_domain_from_url(&approval_url)
            .map_err(|_| HandlerFailure::permanent("quote authorization domain is invalid"))?;
        if !repository
            .remote_domain_allowed(&approval_domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("quote authorization domain policy lookup failed"))?
        {
            return Err(HandlerFailure::permanent(
                "quote authorization domain is not allowed",
            ));
        }
        if !same_url_origin(&approval_url, &target_url) {
            return Err(HandlerFailure::permanent(
                "quoted status does not match the authorization origin",
            ));
        }
        let response = {
            #[cfg(feature = "test-support")]
            if let Some(endpoint) = config.remote_fetch_endpoint {
                fetcher
                    .get_for_test_endpoint(
                        approval_url.clone(),
                        THREAD_ACTIVITYPUB_CONTENT_TYPES,
                        endpoint,
                    )
                    .await
            } else {
                fetcher
                    .get_signed(
                        approval_url.clone(),
                        THREAD_ACTIVITYPUB_CONTENT_TYPES,
                        &signer,
                    )
                    .await
            }
            #[cfg(not(feature = "test-support"))]
            fetcher
                .get_signed(
                    approval_url.clone(),
                    THREAD_ACTIVITYPUB_CONTENT_TYPES,
                    &signer,
                )
                .await
        }
        .map_err(|error| remote_note_fetch_failure(&error))?;
        if !same_url_origin(&response.url, &approval_url) {
            return Err(HandlerFailure::permanent(
                "quote authorization redirected to another origin",
            ));
        }
        Some(
            serde_json::from_slice::<Value>(&response.body)
                .map_err(|_| HandlerFailure::permanent("quote authorization JSON is invalid"))?,
        )
    } else {
        None
    };
    let target_exists = existing_target_local.is_some();
    if !target_exists {
        let embedded_authorization_target = authorization_document
            .as_ref()
            .and_then(|document| document.get("interactionTarget"))
            .filter(|target| target.is_object())
            .cloned();
        let target_document = if let Some(target) = embedded_authorization_target {
            target
        } else {
            let response = {
                #[cfg(feature = "test-support")]
                if let Some(endpoint) = config.remote_fetch_endpoint {
                    fetcher
                        .get_for_test_endpoint(
                            target_url.clone(),
                            THREAD_ACTIVITYPUB_CONTENT_TYPES,
                            endpoint,
                        )
                        .await
                } else {
                    fetcher
                        .get_signed(
                            target_url.clone(),
                            THREAD_ACTIVITYPUB_CONTENT_TYPES,
                            &signer,
                        )
                        .await
                }
                #[cfg(not(feature = "test-support"))]
                fetcher
                    .get_signed(
                        target_url.clone(),
                        THREAD_ACTIVITYPUB_CONTENT_TYPES,
                        &signer,
                    )
                    .await
            }
            .map_err(|error| remote_note_fetch_failure(&error))?;
            if !same_url_origin(&response.url, &target_url) {
                return Err(HandlerFailure::permanent(
                    "quoted status redirected to another origin",
                ));
            }
            serde_json::from_slice::<Value>(&response.body)
                .map_err(|_| HandlerFailure::permanent("quoted status JSON is invalid"))?
        };
        let (target, actor_uri, canonical_target_uri) =
            remote_note_document(&target_document, &references.target_uri)?;
        if canonical_target_uri != references.target_uri {
            return Err(HandlerFailure::permanent(
                "quoted status identity is invalid",
            ));
        }
        validate_fetched_activity_actor(&references.target_uri, &actor_uri)?;
        let account_id =
            resolve_remote_note_author(pool, config, writer, fetcher, &actor_uri, &signer).await?;
        writer
            .apply_remote_note_create(
                account_id,
                &actor_uri,
                &target,
                delivery_target_account_id,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| remote_note_write_failure(&error, "quoted status import failed"))?;
    }
    Ok(Some((references, authorization_document)))
}

#[allow(clippy::too_many_lines)]
pub(super) async fn process_activitypub_inbox(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    claimed_job: &ClaimedJob,
    report_mail_enabled: bool,
) -> Result<(), HandlerFailure> {
    let job = parse_job_arguments(&claimed_job.arguments)
        .map_err(|error| HandlerFailure::permanent(error.to_string()))?;
    let activity =
        parse_activity(&job.body).map_err(|error| HandlerFailure::permanent(error.to_string()))?;
    if matches!(activity, InboxActivity::Unsupported) {
        return Ok(());
    }
    let (actor_uri, nested_actor_uri) = match &activity {
        InboxActivity::CreateVote { actor_uri, .. }
        | InboxActivity::CreateNote { actor_uri, .. }
        | InboxActivity::CreateNoteReference { actor_uri, .. }
        | InboxActivity::UpdateNote { actor_uri, .. }
        | InboxActivity::DeleteNote { actor_uri, .. }
        | InboxActivity::DeleteQuoteAuthorization { actor_uri, .. }
        | InboxActivity::QuoteRequest { actor_uri, .. }
        | InboxActivity::QuoteDecision { actor_uri, .. }
        | InboxActivity::Like { actor_uri, .. }
        | InboxActivity::Announce { actor_uri, .. }
        | InboxActivity::UndoLike { actor_uri, .. }
        | InboxActivity::UndoAnnounce { actor_uri, .. }
        | InboxActivity::Follow { actor_uri, .. }
        | InboxActivity::Flag { actor_uri, .. }
        | InboxActivity::Block { actor_uri, .. }
        | InboxActivity::UpdateActor { actor_uri, .. }
        | InboxActivity::DeleteActor { actor_uri, .. }
        | InboxActivity::UndoReference { actor_uri, .. } => (actor_uri.as_str(), None),
        InboxActivity::Accept {
            actor_uri,
            nested_actor_uri,
            ..
        }
        | InboxActivity::Reject {
            actor_uri,
            nested_actor_uri,
            ..
        }
        | InboxActivity::UndoFollow {
            actor_uri,
            nested_actor_uri,
            ..
        }
        | InboxActivity::UndoBlock {
            actor_uri,
            nested_actor_uri,
            ..
        } => (actor_uri.as_str(), nested_actor_uri.as_deref()),
        InboxActivity::Unsupported => unreachable!("unsupported activities return above"),
    };
    if matches!(
        &activity,
        InboxActivity::UndoFollow { .. } | InboxActivity::UndoBlock { .. }
    ) && nested_actor_uri.is_some_and(|nested| nested != actor_uri)
    {
        return Err(HandlerFailure::permanent(
            "remote Undo actor does not match its embedded Follow",
        ));
    }
    let repository = Repository::from_pool(pool.clone());
    let source_account_id =
        resolve_inbox_actor(&repository, &pool, config, fetcher, &job, actor_uri).await?;
    let writer = WriteRepository::from_pool(pool.clone());
    match activity {
        InboxActivity::CreateVote {
            actor_uri,
            vote_uri,
            question_uri,
            option,
            object,
            activity,
            ..
        } => {
            let outcome = writer
                .apply_remote_poll_vote(
                    source_account_id,
                    &actor_uri,
                    &vote_uri,
                    &question_uri,
                    &option,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote poll vote write failed")
                })?;
            if remote_poll_vote_allows_note_fallback(outcome) {
                let Value::Object(note_object) = &object else {
                    return Err(HandlerFailure::permanent(
                        "remote vote candidate object is not a Note object",
                    ));
                };
                validate_note_object(&actor_uri, note_object)
                    .map_err(|error| HandlerFailure::permanent(error.to_string()))?;
                if !writer
                    .remote_note_is_relevant(
                        source_account_id,
                        &actor_uri,
                        &object,
                        job.delivery_target_account_id,
                        config.origin.as_str(),
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "remote Note relevance check failed")
                    })?
                {
                    return Ok(());
                }
                writer
                    .apply_remote_note_create(
                        source_account_id,
                        &actor_uri,
                        &object,
                        job.delivery_target_account_id,
                        config.origin.as_str(),
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "remote Note Create write failed")
                    })?;
                if activity
                    .get("signature")
                    .is_some_and(|signature| !signature.is_null())
                {
                    writer
                        .record_remote_note_forwarding(&actor_uri, &object, &activity)
                        .await
                        .map_err(|error| {
                            remote_note_write_failure(
                                &error,
                                "remote Note forwarding outbox write failed",
                            )
                        })?;
                }
            }
        }
        InboxActivity::Like {
            activity_uri,
            actor_uri,
            object_uri,
        } => {
            writer
                .apply_remote_like(
                    source_account_id,
                    &actor_uri,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| remote_note_write_failure(&error, "remote Like write failed"))?;
        }
        InboxActivity::UndoLike {
            actor_uri,
            activity_uri,
            object_uri,
        } => {
            writer
                .apply_remote_undo_like(
                    source_account_id,
                    &actor_uri,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Undo Like write failed")
                })?;
        }
        InboxActivity::Announce {
            activity_uri,
            actor_uri,
            object_uri,
            embedded_note,
            to,
            cc,
            published_at,
        } => {
            process_remote_announce(
                &pool,
                config,
                &writer,
                source_account_id,
                &actor_uri,
                &activity_uri,
                &object_uri,
                embedded_note,
                &to,
                &cc,
                published_at.as_deref(),
                job.delivery_target_account_id,
            )
            .await?;
        }
        InboxActivity::UndoAnnounce {
            actor_uri,
            activity_uri,
            object_uri,
        } => {
            writer
                .apply_remote_undo_announce(
                    source_account_id,
                    &actor_uri,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Undo Announce write failed")
                })?;
        }
        InboxActivity::QuoteRequest {
            request_uri,
            actor_uri,
            object_uri,
            instrument,
        } => {
            let instrument_uri = remote_uri_value(Some(&instrument))
                .ok_or_else(|| HandlerFailure::permanent("QuoteRequest instrument has no ID"))?
                .to_owned();
            let import_target = writer
                .remote_quote_request_may_import(
                    source_account_id,
                    &request_uri,
                    &actor_uri,
                    &object_uri,
                    &instrument_uri,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "QuoteRequest import authorization failed")
                })?;
            if let Some((target_status_id, target_account_id)) = import_target {
                let materialized_instrument = if instrument.is_object() {
                    instrument
                } else {
                    match fetch_quote_request_instrument(
                        &repository,
                        config,
                        fetcher,
                        target_account_id,
                        &instrument_uri,
                    )
                    .await
                    {
                        Ok(instrument) => instrument,
                        Err(failure)
                            if failure.disposition == FailureDisposition::Retry
                                && claimed_job.attempt >= claimed_job.max_attempts =>
                        {
                            writer
                                .apply_remote_quote_request(
                                    source_account_id,
                                    &request_uri,
                                    &actor_uri,
                                    &object_uri,
                                    &instrument_uri,
                                    config.origin.as_str(),
                                    job.delivery_target_account_id,
                                )
                                .await
                                .map_err(|error| {
                                    remote_note_write_failure(
                                        &error,
                                        "terminal QuoteRequest rejection failed",
                                    )
                                })?;
                            return Ok(());
                        }
                        Err(failure) => return Err(failure),
                    }
                };
                validate_quote_request_instrument(
                    &writer,
                    &materialized_instrument,
                    &actor_uri,
                    &instrument_uri,
                    target_status_id,
                    config.origin.as_str(),
                )
                .await?;
                writer
                    .apply_remote_quote_request_instrument(
                        source_account_id,
                        &actor_uri,
                        &materialized_instrument,
                        job.delivery_target_account_id,
                        config.origin.as_str(),
                        &request_uri,
                        &object_uri,
                        &instrument_uri,
                        target_status_id,
                        target_account_id,
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "QuoteRequest instrument write failed")
                    })?;
            }
            writer
                .apply_remote_quote_request(
                    source_account_id,
                    &request_uri,
                    &actor_uri,
                    &object_uri,
                    &instrument_uri,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote QuoteRequest write failed")
                })?;
        }
        InboxActivity::QuoteDecision {
            accepted,
            actor_uri,
            request_uri,
            request_actor_uri,
            object_uri,
            instrument_uri,
            result_uri,
        } => {
            let follow_fallback = quote_decision_allows_follow_fallback(
                accepted,
                request_actor_uri.as_deref(),
                object_uri.as_deref(),
                instrument_uri.as_deref(),
            );
            let quote_matched = writer
                .apply_remote_quote_decision(
                    source_account_id,
                    &actor_uri,
                    &request_uri,
                    request_actor_uri.as_deref(),
                    object_uri.as_deref(),
                    instrument_uri.as_deref(),
                    result_uri.as_deref(),
                    accepted,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote quote decision write failed")
                })?;
            if follow_fallback && !quote_matched {
                writer
                    .apply_remote_follow_decision(
                        source_account_id,
                        &request_uri,
                        None,
                        None,
                        true,
                        config.origin.as_str(),
                        job.delivery_target_account_id,
                    )
                    .await
                    .map_err(|_| HandlerFailure::retry("remote Accept write failed"))?;
            }
        }
        InboxActivity::DeleteQuoteAuthorization {
            actor_uri,
            authorization_uri,
            activity,
        } => {
            let forwarding_activity = activity
                .get("signature")
                .is_some_and(|signature| !signature.is_null())
                .then_some(&activity);
            writer
                .apply_remote_quote_authorization_delete(
                    source_account_id,
                    &actor_uri,
                    &authorization_uri,
                    forwarding_activity,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(
                        &error,
                        "remote QuoteAuthorization Delete write failed",
                    )
                })?;
        }
        InboxActivity::CreateNote {
            actor_uri,
            object,
            activity: original_activity,
            ..
        } => {
            if !writer
                .remote_note_is_relevant(
                    source_account_id,
                    &actor_uri,
                    &object,
                    job.delivery_target_account_id,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note relevance check failed")
                })?
            {
                return Ok(());
            }
            let quoting_uri = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| HandlerFailure::permanent("remote Note has no object URI"))?;
            let quote_authorization = fetch_and_import_quote_authorization(
                &pool,
                &repository,
                &writer,
                config,
                fetcher,
                source_account_id,
                job.delivery_target_account_id,
                &object,
            )
            .await?;
            writer
                .apply_remote_note_create(
                    source_account_id,
                    &actor_uri,
                    &object,
                    job.delivery_target_account_id,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note Create write failed")
                })?;
            if let Some((references, Some(document))) = quote_authorization {
                let approval_uri = references
                    .approval_uri
                    .as_deref()
                    .expect("fetched authorization has a canonical URI");
                writer
                    .apply_remote_quote_authorization(
                        source_account_id,
                        quoting_uri,
                        approval_uri,
                        &document,
                        config.origin.as_str(),
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "remote QuoteAuthorization write failed")
                    })?;
            }
            if original_activity
                .get("signature")
                .is_some_and(|signature| !signature.is_null())
            {
                writer
                    .record_remote_note_forwarding(&actor_uri, &object, &original_activity)
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(
                            &error,
                            "remote Note forwarding outbox write failed",
                        )
                    })?;
            }
        }
        InboxActivity::CreateNoteReference {
            activity_uri,
            actor_uri,
            object_uri,
            to,
            cc,
            activity,
        } => {
            schedule_remote_note_resolution(
                &pool,
                source_account_id,
                &activity_uri,
                &actor_uri,
                &object_uri,
                &to,
                &cc,
                job.delivery_target_account_id,
                &activity,
            )
            .await?;
        }
        InboxActivity::UpdateNote {
            actor_uri,
            object,
            activity,
        } => {
            let note_exists = writer
                .remote_note_exists(source_account_id, &actor_uri, &object)
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note existence check failed")
                })?;
            if !note_exists
                && !writer
                    .remote_note_is_relevant(
                        source_account_id,
                        &actor_uri,
                        &object,
                        job.delivery_target_account_id,
                        config.origin.as_str(),
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "remote Note relevance check failed")
                    })?
            {
                return Ok(());
            }
            let quoting_uri = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| HandlerFailure::permanent("remote Note has no object URI"))?;
            let quote_authorization = fetch_and_import_quote_authorization(
                &pool,
                &repository,
                &writer,
                config,
                fetcher,
                source_account_id,
                job.delivery_target_account_id,
                &object,
            )
            .await?;
            let updated = writer
                .apply_remote_note_update(
                    source_account_id,
                    &actor_uri,
                    &object,
                    job.delivery_target_account_id,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note Update write failed")
                })?;
            if let Some((references, Some(document))) = quote_authorization {
                let approval_uri = references
                    .approval_uri
                    .as_deref()
                    .expect("fetched authorization has a canonical URI");
                writer
                    .apply_remote_quote_authorization(
                        source_account_id,
                        quoting_uri,
                        approval_uri,
                        &document,
                        config.origin.as_str(),
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "remote QuoteAuthorization write failed")
                    })?;
            }
            if note_exists
                && updated.is_some()
                && activity
                    .get("signature")
                    .is_some_and(|signature| !signature.is_null())
            {
                writer
                    .record_remote_note_forwarding(&actor_uri, &object, &activity)
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(
                            &error,
                            "remote Note Update forwarding outbox write failed",
                        )
                    })?;
            }
        }
        InboxActivity::DeleteNote {
            actor_uri,
            object_uri,
            atom_uri,
            activity,
        } => {
            // Mastodon accepts both an embedded QuoteAuthorization and its bare URI in Delete.
            // Try the tightly actor-bound authorization lookup before interpreting a scalar URI
            // as a Note; a miss is side-effect free and falls through to ordinary deletion.
            let forwarding_activity = activity
                .get("signature")
                .is_some_and(|signature| !signature.is_null())
                .then_some(&activity);
            if writer
                .apply_remote_quote_authorization_delete(
                    source_account_id,
                    &actor_uri,
                    &object_uri,
                    forwarding_activity,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(
                        &error,
                        "remote QuoteAuthorization Delete write failed",
                    )
                })?
            {
                return Ok(());
            }
            if activity
                .get("signature")
                .is_some_and(|signature| !signature.is_null())
            {
                writer
                    .record_remote_note_delete_forwarding(
                        &actor_uri,
                        &object_uri,
                        atom_uri.as_deref(),
                        &activity,
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(
                            &error,
                            "remote Note Delete forwarding outbox write failed",
                        )
                    })?;
            }
            let media = writer
                .remote_note_media_metadata(
                    source_account_id,
                    &actor_uri,
                    &object_uri,
                    atom_uri.as_deref(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note media lookup failed")
                })?;
            if let Some(media_root) = config.media_root.as_ref() {
                remove_paperclip_files(media_root, &media)?;
            }
            writer
                .apply_remote_note_delete(
                    source_account_id,
                    &actor_uri,
                    &object_uri,
                    atom_uri.as_deref(),
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note Delete write failed")
                })?;
        }
        InboxActivity::Follow {
            activity_uri,
            object_uri,
            ..
        } => {
            let outcome = writer
                .apply_remote_follow(
                    source_account_id,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Follow write failed"))?;
            if let Some(outcome) = outcome {
                match outcome {
                    RemoteFollowOutcome::Applied(outcome) => {
                        if !outcome.request {
                            schedule_remote_follow_accept(
                                &pool,
                                &repository,
                                config,
                                source_account_id,
                                outcome.recipient_account_id,
                                outcome.activity_id,
                                &activity_uri,
                            )
                            .await?;
                        }
                    }
                    RemoteFollowOutcome::Rejected {
                        recipient_account_id,
                    } => {
                        schedule_remote_follow_reject(
                            &pool,
                            &repository,
                            config,
                            source_account_id,
                            recipient_account_id,
                            &activity_uri,
                        )
                        .await?;
                    }
                }
            }
        }
        InboxActivity::UndoFollow {
            follow_uri,
            target_uri,
            ..
        } => {
            writer
                .apply_remote_undo_follow(
                    source_account_id,
                    &follow_uri,
                    target_uri.as_deref(),
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Undo Follow write failed"))?;
        }
        InboxActivity::Block {
            activity_uri,
            object_uri,
            ..
        } => {
            writer
                .apply_remote_block(
                    source_account_id,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Block write failed"))?;
        }
        InboxActivity::Flag {
            activity_uri,
            object_uris,
            comment,
            ..
        } => {
            writer
                .create_remote_report(
                    source_account_id,
                    &object_uris,
                    &comment,
                    activity_uri.as_deref(),
                    config.origin.as_str(),
                    &config.local_domain,
                    report_mail_enabled,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Flag report write failed")
                })?;
        }
        InboxActivity::UpdateActor { actor_uri, object } => {
            writer
                .apply_remote_actor_update(source_account_id, &actor_uri, &object)
                .await
                .map_err(|_| HandlerFailure::retry("remote actor Update write failed"))?;
        }
        InboxActivity::DeleteActor {
            actor_uri,
            object_uri,
        } => {
            if actor_uri != object_uri {
                return Err(HandlerFailure::permanent(
                    "remote actor Delete object does not match its actor",
                ));
            }
            let media = writer
                .remote_actor_media_metadata(source_account_id, &actor_uri)
                .await
                .map_err(|_| HandlerFailure::retry("remote actor media lookup failed"))?;
            if let Some(media_root) = config.media_root.as_ref() {
                remove_paperclip_files(media_root, &media)?;
            }
            writer
                .apply_remote_actor_delete(
                    source_account_id,
                    &actor_uri,
                    config.origin.as_str(),
                    None,
                    None,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote actor Delete write failed"))?;
        }
        InboxActivity::UndoBlock {
            block_uri,
            target_uri,
            ..
        } => {
            writer
                .apply_remote_undo_block(
                    source_account_id,
                    &block_uri,
                    target_uri.as_deref(),
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Undo Block write failed"))?;
        }
        InboxActivity::Accept {
            follow_uri,
            target_uri,
            nested_actor_uri,
            ..
        } => {
            writer
                .apply_remote_follow_decision(
                    source_account_id,
                    &follow_uri,
                    target_uri.as_deref(),
                    nested_actor_uri.as_deref(),
                    true,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Accept write failed"))?;
        }
        InboxActivity::Reject {
            actor_uri,
            follow_uri,
            target_uri,
            nested_actor_uri,
        } => {
            let quote_matched = writer
                .apply_remote_quote_decision(
                    source_account_id,
                    &actor_uri,
                    &follow_uri,
                    nested_actor_uri.as_deref(),
                    target_uri.as_deref(),
                    None,
                    None,
                    false,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote quote Reject write failed")
                })?;
            if !quote_matched {
                writer
                    .apply_remote_follow_decision(
                        source_account_id,
                        &follow_uri,
                        target_uri.as_deref(),
                        nested_actor_uri.as_deref(),
                        false,
                        config.origin.as_str(),
                        job.delivery_target_account_id,
                    )
                    .await
                    .map_err(|_| HandlerFailure::retry("remote Reject write failed"))?;
            }
        }
        InboxActivity::UndoReference {
            actor_uri,
            object_uri,
        } => {
            match writer
                .remote_undo_reference_kind(source_account_id, &object_uri)
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Undo reference lookup failed")
                })? {
                RemoteUndoReferenceKind::Follow => writer
                    .apply_remote_undo_follow(
                        source_account_id,
                        &object_uri,
                        None,
                        config.origin.as_str(),
                        job.delivery_target_account_id,
                    )
                    .await
                    .map_err(|_| {
                        HandlerFailure::retry("remote Undo reference follow write failed")
                    })?,
                RemoteUndoReferenceKind::Block => writer
                    .apply_remote_undo_block(
                        source_account_id,
                        &object_uri,
                        None,
                        config.origin.as_str(),
                        job.delivery_target_account_id,
                    )
                    .await
                    .map_err(|_| {
                        HandlerFailure::retry("remote Undo reference block write failed")
                    })?,
                RemoteUndoReferenceKind::Announce | RemoteUndoReferenceKind::Unknown => writer
                    .apply_remote_undo_announce_reference(
                        source_account_id,
                        &actor_uri,
                        &object_uri,
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(
                            &error,
                            "remote Undo reference interaction write failed",
                        )
                    })?,
            }
        }
        InboxActivity::Unsupported => unreachable!("unsupported activities return above"),
    }
    Ok(())
}

pub(super) fn remote_note_write_failure(error: &WriteError, message: &str) -> HandlerFailure {
    match error {
        &WriteError::Conflict
        | &WriteError::InvalidInput(_)
        | &WriteError::NotFound
        | &WriteError::Unauthorized
        | &WriteError::Forbidden
        | &WriteError::RateLimited
        | &WriteError::Validation(_) => HandlerFailure::permanent(message),
        &WriteError::Sqlx(_) | &WriteError::Job(_) | &WriteError::Filesystem(_) => {
            HandlerFailure::retry(message)
        }
    }
}
