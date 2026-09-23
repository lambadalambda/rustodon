//! Status and account distribution to remote followers and mentioned inboxes.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

pub(super) struct QuoteRequestDistribution {
    pub(super) quote_id: i64,
    pub(super) request_uri: String,
    pub(super) quoted_status_id: i64,
    pub(super) quoted_status_uri: String,
    pub(super) quoted_status_url: String,
    pub(super) quoted_account_id: i64,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn distribute_status(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    status_id: i64,
    activity_type: &str,
    edited_at_micros: Option<i64>,
    poll_updated_at_micros: Option<i64>,
    quote_updated_at_micros: Option<i64>,
    update_kind: Option<&str>,
    update_version_micros: Option<i64>,
    explicit_recipient_ids: &[i64],
    quote_request: Option<&QuoteRequestDistribution>,
) -> Result<(), HandlerFailure> {
    let is_quote_request = activity_type == "QuoteRequest";
    let is_delete = match activity_type {
        "Create" | "Update" | "QuoteRequest" => false,
        "Delete" => true,
        _ => {
            return Err(HandlerFailure::permanent(
                "status distribution activity type is unsupported",
            ));
        }
    };
    if is_quote_request != quote_request.is_some() {
        return Err(HandlerFailure::permanent(
            "status quote-request distribution arguments are invalid",
        ));
    }
    let requested_edited_at = edited_at_micros
        .map(|value| {
            DateTime::<Utc>::from_timestamp_micros(value)
                .map(|timestamp| timestamp.naive_utc())
                .ok_or_else(|| HandlerFailure::permanent("status edit timestamp is invalid"))
        })
        .transpose()?;
    let requested_poll_updated_at = poll_updated_at_micros
        .map(|value| {
            DateTime::<Utc>::from_timestamp_micros(value)
                .map(|timestamp| timestamp.naive_utc())
                .ok_or_else(|| HandlerFailure::permanent("poll update timestamp is invalid"))
        })
        .transpose()?;
    let requested_update_version = update_version_micros
        .map(|value| {
            DateTime::<Utc>::from_timestamp_micros(value)
                .map(|timestamp| timestamp.naive_utc())
                .ok_or_else(|| HandlerFailure::permanent("status update version is invalid"))
        })
        .transpose()?;
    let explicit_update_kind = update_kind.is_some();
    let resolved_update_kind = match update_kind {
        Some(value) => StatusUpdateKind::parse(value).ok_or_else(|| {
            HandlerFailure::permanent("status distribution update kind is unsupported")
        })?,
        None if requested_poll_updated_at.is_some() && requested_edited_at.is_none() => {
            StatusUpdateKind::Poll
        }
        None if requested_poll_updated_at.is_some()
            && update_version_micros == poll_updated_at_micros =>
        {
            StatusUpdateKind::Poll
        }
        None if requested_poll_updated_at.is_some()
            && update_version_micros == edited_at_micros =>
        {
            StatusUpdateKind::Status
        }
        None if requested_poll_updated_at.is_some() => StatusUpdateKind::PollRepair,
        None => StatusUpdateKind::Status,
    };
    let poll_update = resolved_update_kind.has_poll_reach();
    let repository = Repository::from_pool(pool.clone());
    let status_result = if is_delete {
        repository.status_including_deleted(status_id).await
    } else {
        repository.status(status_id).await
    };
    let Some(status) =
        status_result.map_err(|_| HandlerFailure::retry("status delivery lookup failed"))?
    else {
        return Ok(());
    };
    if status.local != Some(true) {
        return Ok(());
    }
    if status.reblog_of_id.is_some() && activity_type == "Update" {
        return Ok(());
    }
    if status.reblog_of_id.is_some() && is_quote_request {
        return Err(HandlerFailure::permanent(
            "quote-request instrument cannot be a reblog",
        ));
    }
    let current_edited_at = if resolved_update_kind == StatusUpdateKind::InteractionPolicy {
        status.updated_at
    } else {
        status.edited_at.unwrap_or(status.updated_at)
    };
    let (requested_edited_at, requested_poll_updated_at, selected_update_version) = if activity_type
        == "Update"
    {
        let current_poll_updated_at = if let Some(poll_id) = status.poll_id {
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT updated_at FROM polls WHERE id = $1")
                .bind(poll_id)
                .fetch_optional(&pool)
                .await
                .map_err(|_| HandlerFailure::retry("poll update fence lookup failed"))?
        } else {
            None
        };
        let version_decision = if resolved_update_kind == StatusUpdateKind::Quote {
            let current_quote_updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
                "SELECT updated_at FROM quotes WHERE status_id = $1 ORDER BY id LIMIT 1",
            )
            .bind(status_id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| HandlerFailure::retry("quote update fence lookup failed"))?;
            quote_update_versions(
                current_edited_at,
                current_poll_updated_at,
                current_quote_updated_at,
                requested_edited_at,
                requested_poll_updated_at,
                quote_updated_at_micros,
                requested_update_version,
            )
            .map_or(
                StatusUpdateVersionDecision::Stale,
                |(edited_at, poll_updated_at, update_version)| {
                    StatusUpdateVersionDecision::Deliver(edited_at, poll_updated_at, update_version)
                },
            )
        } else {
            status_update_version_decision(
                current_edited_at,
                current_poll_updated_at,
                requested_edited_at,
                requested_poll_updated_at,
                resolved_update_kind,
                explicit_update_kind || resolved_update_kind.is_repair(),
            )
        };
        match version_decision {
            StatusUpdateVersionDecision::Deliver(edited_at, poll_updated_at, update_version) => {
                (Some(edited_at), poll_updated_at, Some(update_version))
            }
            StatusUpdateVersionDecision::Repair if resolved_update_kind.is_repair() => {
                let Some((edited_at, poll_updated_at, update_version)) = status_update_versions(
                    current_edited_at,
                    current_poll_updated_at,
                    Some(current_edited_at),
                    current_poll_updated_at,
                    resolved_update_kind,
                    true,
                ) else {
                    queue_status_snapshot_repair(
                        &pool,
                        status_id,
                        status.poll_id,
                        current_edited_at,
                        current_poll_updated_at,
                    )
                    .await?;
                    return Ok(());
                };
                (Some(edited_at), poll_updated_at, Some(update_version))
            }
            StatusUpdateVersionDecision::Repair => {
                queue_status_snapshot_repair(
                    &pool,
                    status_id,
                    status.poll_id,
                    current_edited_at,
                    current_poll_updated_at,
                )
                .await?;
                return Ok(());
            }
            StatusUpdateVersionDecision::Stale => return Ok(()),
        }
    } else {
        (requested_edited_at, requested_poll_updated_at, None)
    };
    let selected_update_version_micros = selected_update_version
        .map(|value| value.and_utc().timestamp_micros())
        .or(update_version_micros);
    let account = repository
        .account(status.account_id)
        .await
        .map_err(|_| HandlerFailure::retry("status author lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("status author is missing"))?;
    let media = if is_delete {
        Vec::new()
    } else {
        repository
            .media_attachments(status_id)
            .await
            .map_err(|_| HandlerFailure::retry("status media lookup failed"))?
    };
    let status_stat = if is_delete {
        None
    } else {
        repository
            .status_stat(status_id)
            .await
            .map_err(|_| HandlerFailure::retry("status statistics lookup failed"))?
    };
    let mention_rows = repository
        .mentions(status_id)
        .await
        .map_err(|_| HandlerFailure::retry("status mention lookup failed"))?;
    let mut mentions = Vec::with_capacity(mention_rows.len());
    let mut mentioned_recipient_ids = Vec::new();
    for mention in mention_rows {
        if let Some(target) = repository
            .account(mention.account_id)
            .await
            .map_err(|_| HandlerFailure::retry("status mention target lookup failed"))?
        {
            if target.domain.is_some() && target.protocol.0 != 1 {
                continue;
            }
            if target.domain.is_some() {
                mentioned_recipient_ids.push(target.id);
            }
            if !mention.silent {
                mentions.push((mention, target));
            }
        }
    }
    let activity = if let Some(reblog_of_id) = status.reblog_of_id {
        let target_status = repository
            .status_including_deleted(reblog_of_id)
            .await
            .map_err(|_| HandlerFailure::retry("reblog target lookup failed"))?
            .ok_or_else(|| HandlerFailure::permanent("reblog target is missing"))?;
        let target_account = repository
            .account(target_status.account_id)
            .await
            .map_err(|_| HandlerFailure::retry("reblog target author lookup failed"))?
            .ok_or_else(|| HandlerFailure::permanent("reblog target author is missing"))?;
        let actor_uri = activitypub::actor_url(&config.origin, &account);
        let announce_uri = activitypub::status_uri(&config.origin, &account, &status);
        let object_uri = activitypub::status_uri(&config.origin, &target_account, &target_status);
        let (to, mut cc) = activitypub::local_announce_audience(status.visibility, &actor_uri);
        if let Value::Array(values) = &mut cc {
            values.push(Value::String(activitypub::actor_url(
                &config.origin,
                &target_account,
            )));
        }
        if is_delete {
            let undo_uri = format!("{actor_uri}#announces/{}/undo", status.id);
            activitypub::undo_announce_with_uris(
                &undo_uri,
                &actor_uri,
                &announce_uri,
                status.created_at,
                &object_uri,
                to,
                cc,
            )
        } else {
            let object = if status.visibility == StatusVisibility::Private
                && target_status.local == Some(true)
                && target_account.id == account.id
            {
                let target_media = repository
                    .media_attachments(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost media lookup failed"))?;
                let target_mention_rows = repository
                    .mentions(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost mention lookup failed"))?;
                let mut target_mentions = Vec::with_capacity(target_mention_rows.len());
                for mention in target_mention_rows {
                    if mention.silent {
                        continue;
                    }
                    if let Some(target) = repository
                        .account(mention.account_id)
                        .await
                        .map_err(|_| HandlerFailure::retry("private boost target lookup failed"))?
                    {
                        if target.domain.is_some() && target.protocol.0 != 1 {
                            continue;
                        }
                        target_mentions.push((mention, target));
                    }
                }
                let target_hashtags = repository
                    .tags(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost hashtag lookup failed"))?
                    .into_iter()
                    .map(|tag| (tag.name, tag.display_name.unwrap_or_default()))
                    .collect::<Vec<_>>();
                let target_emojis = repository
                    .activitypub_status_emojis(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost emoji lookup failed"))?;
                let target_stat = repository
                    .status_stat(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost statistics lookup failed"))?;
                let (quoted_link, quoted_identifier, quote_authorization) =
                    activitypub_quote_parts(&repository, config, reblog_of_id).await?;
                let mut object = activitypub::note(
                    &config.origin,
                    &config.local_domain,
                    &target_status,
                    &target_account,
                    &config.media_root_url,
                    &target_media,
                    &target_mentions,
                    &target_hashtags,
                    &target_emojis,
                    quoted_link.as_deref(),
                    None,
                    None,
                    None,
                    quoted_identifier.as_deref(),
                    quote_authorization.as_deref(),
                    None,
                    target_stat
                        .as_ref()
                        .map_or(0, |stats| stats.favourites_count),
                    target_stat.as_ref().map_or(0, |stats| stats.reblogs_count),
                );
                if let Some(poll_id) = target_status.poll_id {
                    let loaded_poll = repository
                        .poll(poll_id)
                        .await
                        .map_err(|_| HandlerFailure::retry("private boost poll lookup failed"))?
                        .ok_or_else(|| {
                            HandlerFailure::permanent("private boost poll is missing")
                        })?;
                    object = activitypub::question(object, &loaded_poll, Utc::now().naive_utc());
                }
                object
            } else {
                Value::String(object_uri)
            };
            activitypub::announce_with_object(
                &announce_uri,
                &actor_uri,
                status.created_at,
                object,
                to,
                cc,
            )
        }
    } else if is_delete {
        let object_uri = activitypub::status_uri(&config.origin, &account, &status);
        let delete_uri = format!("{object_uri}#delete");
        let atom_uri = status.uri.as_deref().unwrap_or(&object_uri);
        activitypub::delete_with_uris(
            &delete_uri,
            &activitypub::actor_url(&config.origin, &account),
            &object_uri,
            atom_uri,
        )
    } else {
        let hashtags = repository
            .tags(status_id)
            .await
            .map_err(|_| HandlerFailure::retry("status hashtag lookup failed"))?
            .into_iter()
            .map(|tag| (tag.name, tag.display_name.unwrap_or_default()))
            .collect::<Vec<_>>();
        let (in_reply_to_url, in_reply_to_atom_uri) = if let Some(parent_id) = status.in_reply_to_id
        {
            let parent = repository
                .status_including_deleted(parent_id)
                .await
                .map_err(|_| HandlerFailure::retry("reply target lookup failed"))?;
            if let Some(parent) = parent {
                repository
                    .account(parent.account_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("reply author lookup failed"))?
                    .map_or((String::new(), None), |parent_account| {
                        let atom_uri = if parent_account.domain.is_none() {
                            Some(parent.uri.clone().unwrap_or_else(|| {
                                format!(
                                    "tag:{},{}:objectId={}:objectType=Status",
                                    config.local_domain,
                                    parent.created_at.date(),
                                    parent.id
                                )
                            }))
                        } else {
                            parent.uri.clone()
                        };
                        (
                            activitypub::status_uri(&config.origin, &parent_account, &parent),
                            atom_uri,
                        )
                    })
            } else {
                (String::new(), None)
            }
        } else {
            (String::new(), None)
        };
        let in_reply_to_url = (!in_reply_to_url.is_empty()).then_some(in_reply_to_url);
        let conversation = match status.conversation_id {
            Some(conversation_id) => repository
                .conversation(conversation_id)
                .await
                .map_err(|_| HandlerFailure::retry("status conversation lookup failed"))?
                .and_then(|conversation| conversation.uri),
            None => None,
        };
        let (quoted_link, quoted_identifier, quote_authorization) =
            if let Some(quote_request) = quote_request {
                (
                    Some(quote_request.quoted_status_url.clone()),
                    Some(quote_request.quoted_status_uri.clone()),
                    None,
                )
            } else {
                activitypub_quote_parts(&repository, config, status_id).await?
            };
        let emojis = repository
            .activitypub_status_emojis(status_id)
            .await
            .map_err(|_| HandlerFailure::retry("status emoji lookup failed"))?;
        let mut object = activitypub::note(
            &config.origin,
            &config.local_domain,
            &status,
            &account,
            &config.media_root_url,
            &media,
            &mentions,
            &hashtags,
            &emojis,
            quoted_link.as_deref(),
            in_reply_to_url.as_deref(),
            in_reply_to_atom_uri.as_deref(),
            conversation.as_deref(),
            quoted_identifier.as_deref(),
            quote_authorization.as_deref(),
            None,
            status_stat
                .as_ref()
                .map_or(0, |stats| stats.favourites_count),
            status_stat.as_ref().map_or(0, |stats| stats.reblogs_count),
        );
        if let Some(poll_id) = status.poll_id {
            let loaded_poll = repository
                .poll(poll_id)
                .await
                .map_err(|_| HandlerFailure::retry("status poll lookup failed"))?
                .ok_or_else(|| HandlerFailure::permanent("status poll is missing"))?;
            object = activitypub::question(object, &loaded_poll, Utc::now().naive_utc());
        }
        if let Some(quote_request) = quote_request {
            activitypub::quote_request_with_uris(
                &quote_request.request_uri,
                &activitypub::actor_url(&config.origin, &account),
                &quote_request.quoted_status_uri,
                object,
            )
        } else if activity_type == "Update" {
            let object_uri = object["id"]
                .as_str()
                .ok_or_else(|| HandlerFailure::permanent("status Note has no ID"))?
                .to_owned();
            let update_version = selected_update_version
                .ok_or_else(|| HandlerFailure::permanent("status Update has no version"))?;
            if !poll_update {
                object["updated"] = json!(activitypub::timestamp(update_version));
            }
            let update_uri = if resolved_update_kind.is_repair() {
                status_snapshot_repair_activity_id(
                    &object_uri,
                    requested_edited_at
                        .ok_or_else(|| HandlerFailure::permanent("repair has no status version"))?,
                    requested_poll_updated_at,
                )
            } else {
                activitypub::update_activity_id(&object_uri, update_version)
            };
            activitypub::update_with_uris(
                &update_uri,
                &activitypub::actor_url(&config.origin, &account),
                update_version,
                object,
            )
        } else {
            activitypub::create(&config.origin, &account, &status, object)
        }
    };
    let include_unsafe_reach = is_delete;
    let recipient_ids = if let Some(quote_request) = quote_request {
        BTreeSet::from([quote_request.quoted_account_id])
    } else {
        let follower_ids = if matches!(
            status.visibility,
            StatusVisibility::Public | StatusVisibility::Unlisted | StatusVisibility::Private
        ) {
            repository
                .activitypub_remote_follower_ids(status.account_id, include_unsafe_reach)
                .await
                .map_err(|_| HandlerFailure::retry("remote follower lookup failed"))?
        } else {
            Vec::new()
        };
        let mut recipient_ids = follower_ids.into_iter().collect::<BTreeSet<_>>();
        recipient_ids.extend(explicit_recipient_ids.iter().copied());
        let reached_account_ids = if status.reblog_of_id.is_some() {
            repository
                .activitypub_reblog_target_account_ids(status_id, include_unsafe_reach)
                .await
                .map_err(|_| HandlerFailure::retry("reblog target reach lookup failed"))?
        } else {
            repository
                .activitypub_status_reach_account_ids(status_id, include_unsafe_reach)
                .await
                .map_err(|_| HandlerFailure::retry("status reach lookup failed"))?
        };
        recipient_ids.extend(reached_account_ids);
        if poll_update {
            recipient_ids.extend(
                repository
                    .activitypub_poll_voter_account_ids(status_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("poll voter reach lookup failed"))?,
            );
        }
        recipient_ids.extend(mentioned_recipient_ids);
        recipient_ids
    };
    let mut inboxes = BTreeMap::new();
    for recipient_id in recipient_ids {
        let Some(follower) = repository
            .account(recipient_id)
            .await
            .map_err(|_| HandlerFailure::retry("remote follower account lookup failed"))?
        else {
            continue;
        };
        let Some(domain) = follower.domain.as_deref() else {
            continue;
        };
        if follower.protocol.0 != 1 {
            continue;
        }
        if follower.suspended_at.is_some() && !include_unsafe_reach {
            continue;
        }
        if !repository
            .remote_domain_allowed(domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("remote domain policy lookup failed"))?
        {
            continue;
        }
        let inbox_url = if is_quote_request || follower.shared_inbox_url.is_empty() {
            follower.inbox_url
        } else {
            follower.shared_inbox_url
        };
        if !inbox_url.is_empty() {
            inboxes
                .entry(inbox_url)
                .or_insert_with(|| domain.to_owned());
        }
    }
    if !is_quote_request && status.visibility == StatusVisibility::Public {
        let relay_inboxes = repository
            .activitypub_relay_inboxes()
            .await
            .map_err(|_| HandlerFailure::retry("status relay lookup failed"))?;
        for inbox_url in relay_inboxes {
            let Ok(parsed) = Url::parse(&inbox_url) else {
                continue;
            };
            let Some(host) = parsed.host_str() else {
                continue;
            };
            let authority = parsed
                .port()
                .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
            let Ok(domain) = canonical_remote_domain(&authority) else {
                continue;
            };
            if repository
                .remote_domain_allowed(&domain, config.limited_federation)
                .await
                .map_err(|_| HandlerFailure::retry("status relay policy lookup failed"))?
            {
                inboxes.insert(inbox_url, domain);
            }
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("delivery outbox transaction failed"))?;
    let activity_id = activity["id"]
        .as_str()
        .ok_or_else(|| HandlerFailure::permanent("status activity has no ID"))?;
    if is_quote_request {
        let quoted_status_id = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT quoted_status_id FROM quotes
              WHERE id = $1 AND status_id = $2 AND quoted_status_id = $3
                AND state = 0 AND activity_uri = $4
              FOR UPDATE",
        )
        .bind(
            quote_request
                .expect("QuoteRequest has distribution metadata")
                .quote_id,
        )
        .bind(status_id)
        .bind(
            quote_request
                .expect("QuoteRequest has distribution metadata")
                .quoted_status_id,
        )
        .bind(activity_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| HandlerFailure::retry("quote-request lifecycle fence failed"))?
        .flatten();
        let relationship_is_live = if let Some(quoted_status_id) = quoted_status_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                   SELECT 1 FROM statuses quoting
                   JOIN statuses quoted ON quoted.id = $2
                  WHERE quoting.id = $1 AND quoting.local IS TRUE
                    AND quoting.deleted_at IS NULL AND quoted.deleted_at IS NULL)",
            )
            .bind(status_id)
            .bind(quoted_status_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| HandlerFailure::retry("quote-request status fence failed"))?
        } else {
            false
        };
        if !relationship_is_live {
            transaction
                .commit()
                .await
                .map_err(|_| HandlerFailure::retry("quote-request fence commit failed"))?;
            return Ok(());
        }
    }
    for (inbox_url, remote_domain) in inboxes {
        let logical_key = match activity_type {
            "Create" => delivery_logical_key(status_id, &inbox_url),
            "Update" => update_delivery_logical_key(
                status_id,
                activity_id,
                selected_update_version_micros.expect("Update activity has an edit version"),
                &inbox_url,
            ),
            "Delete" => delete_delivery_logical_key(status_id, &inbox_url),
            "QuoteRequest" => {
                format!("activitypub:quote-request:{activity_id}:{inbox_url}")
            }
            _ => unreachable!("activity type was validated above"),
        };
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "status_id": status_id,
                "source_account_id": account.id,
                "inbox_url": inbox_url,
                "body": activity.clone(),
                "activity_type": activity_type,
                "update_kind": (activity_type == "Update").then_some(resolved_update_kind.as_str()),
                "update_version_micros": selected_update_version_micros,
                "edited_at_micros": requested_edited_at.map(|value| value.and_utc().timestamp_micros()),
                "poll_updated_at_micros": requested_poll_updated_at.map(|value| value.and_utc().timestamp_micros()),
                "quote_updated_at_micros": quote_updated_at_micros,
                "quote_delivery_kind": is_quote_request.then_some("request"),
                "quote_request_uri": quote_request.map(|request| request.request_uri.as_str()),
                "quote_id": quote_request.map(|request| request.quote_id),
                "quoting_status_id": is_quote_request.then_some(status_id),
                "quoted_status_id": quote_request.map(|request| request.quoted_status_id),
                "remote_domain": remote_domain
            }),
        )
        .logical_key(logical_key);
        record_outbox_once_in(&mut transaction, &delivery)
            .await
            .map_err(|_| HandlerFailure::retry("delivery outbox write failed"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("delivery outbox commit failed"))?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) async fn distribute_account_update(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    account_id: i64,
    updated_at_micros: i64,
) -> Result<(), HandlerFailure> {
    let requested_updated_at = DateTime::<Utc>::from_timestamp_micros(updated_at_micros)
        .map(|timestamp| timestamp.naive_utc())
        .ok_or_else(|| HandlerFailure::permanent("account update timestamp is invalid"))?;
    let repository = Repository::from_pool(pool.clone());
    let Some(account) = repository
        .account(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account update lookup failed"))?
    else {
        return Ok(());
    };
    if account.domain.is_some() || account.updated_at != requested_updated_at {
        return Ok(());
    }
    let hashtags = repository
        .activitypub_account_hashtags(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account update hashtag lookup failed"))?;
    let emojis = repository
        .activitypub_account_emojis(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account update emoji lookup failed"))?;
    let activity = activitypub::update_actor(
        &config.origin,
        &config.local_domain,
        &config.media_root_url,
        &account,
        &hashtags,
        &emojis,
    );
    let recipient_ids = repository
        .activitypub_account_reach_account_ids(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account update reach lookup failed"))?;
    let relay_inboxes = repository
        .activitypub_relay_inboxes()
        .await
        .map_err(|_| HandlerFailure::retry("account update relay lookup failed"))?;
    let mut inboxes = BTreeMap::new();
    for recipient_id in recipient_ids {
        let Some(recipient) = repository
            .account(recipient_id)
            .await
            .map_err(|_| HandlerFailure::retry("account update recipient lookup failed"))?
        else {
            continue;
        };
        let Some(domain) = recipient.domain.as_deref() else {
            continue;
        };
        if recipient.protocol.0 != 1 {
            continue;
        }
        if !repository
            .remote_domain_allowed(domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("account update domain policy lookup failed"))?
        {
            continue;
        }
        let inbox_url = if recipient.shared_inbox_url.is_empty() {
            recipient.inbox_url
        } else {
            recipient.shared_inbox_url
        };
        if !inbox_url.is_empty() {
            inboxes
                .entry(inbox_url)
                .or_insert_with(|| domain.to_owned());
        }
    }
    for inbox_url in relay_inboxes {
        let Ok(parsed) = Url::parse(&inbox_url) else {
            continue;
        };
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let authority = parsed
            .port()
            .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
        let Ok(domain) = canonical_remote_domain(&authority) else {
            continue;
        };
        if repository
            .remote_domain_allowed(&domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("account update relay policy lookup failed"))?
        {
            inboxes.insert(inbox_url, domain);
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let activity_id = activity["id"]
        .as_str()
        .ok_or_else(|| HandlerFailure::permanent("account update has no ID"))?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("account update outbox transaction failed"))?;
    for (inbox_url, remote_domain) in inboxes {
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": activity,
                "activity_type": "AccountUpdate",
                "updated_at_micros": updated_at_micros
            }),
        )
        .logical_key(account_update_delivery_logical_key(
            account_id,
            activity_id,
            updated_at_micros,
            &inbox_url,
        ));
        record_outbox_once_in(&mut transaction, &delivery)
            .await
            .map_err(|_| HandlerFailure::retry("account update delivery outbox write failed"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("account update delivery outbox commit failed"))?;
    Ok(())
}

pub(super) async fn distribute_account_delete(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    account_id: i64,
    actor_uri: &str,
) -> Result<(), HandlerFailure> {
    let repository = Repository::from_pool(pool.clone());
    let Some(account) = repository
        .account(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account deletion lookup failed"))?
    else {
        return Ok(());
    };
    if account.domain.is_some() || account.suspended_at.is_none() {
        return Ok(());
    }

    let mut inboxes = BTreeMap::new();
    let remote_inboxes = repository
        .activitypub_remote_inboxes()
        .await
        .map_err(|_| HandlerFailure::retry("account deletion recipient lookup failed"))?;
    for (inbox_url, domain) in remote_inboxes {
        if repository
            .remote_domain_allowed(&domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("account deletion domain policy lookup failed"))?
            && !inbox_url.is_empty()
        {
            inboxes.entry(inbox_url).or_insert(domain);
        }
    }
    let relay_inboxes = repository
        .activitypub_relay_inboxes()
        .await
        .map_err(|_| HandlerFailure::retry("account deletion relay lookup failed"))?;
    for inbox_url in relay_inboxes {
        let Ok(parsed) = Url::parse(&inbox_url) else {
            continue;
        };
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let authority = parsed
            .port()
            .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
        let Ok(domain) = canonical_remote_domain(&authority) else {
            continue;
        };
        if repository
            .remote_domain_allowed(&domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("account deletion relay policy lookup failed"))?
        {
            inboxes.entry(inbox_url).or_insert(domain);
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let activity = activitypub::delete_actor_with_uris(&format!("{actor_uri}#delete"), actor_uri);
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("account deletion outbox transaction failed"))?;
    if !account_delete_delivery_is_current(&mut transaction, account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account deletion state lookup failed"))?
    {
        return Ok(());
    }
    for (inbox_url, remote_domain) in inboxes {
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": activity,
                "activity_type": "AccountDelete"
            }),
        )
        .logical_key(activitypub::delete_actor_delivery_logical_key(
            actor_uri, &inbox_url,
        ));
        record_outbox_once_in(&mut transaction, &delivery)
            .await
            .map_err(|_| HandlerFailure::retry("account deletion delivery outbox write failed"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("account deletion delivery outbox commit failed"))?;
    Ok(())
}
