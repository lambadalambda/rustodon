//! Remote `ActivityPub` writes: actors, notes, interactions, quotes and relationships.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

#[allow(clippy::missing_errors_doc)]
impl WriteRepository {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn apply_remote_quote_decision(
        &self,
        source_account_id: i64,
        actor_uri: &str,
        request_uri: &str,
        request_actor_uri: Option<&str>,
        quoted_status_uri: Option<&str>,
        instrument_uri: Option<&str>,
        result_uri: Option<&str>,
        accepted: bool,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<bool, WriteError> {
        if accepted {
            let result_uri = result_uri.ok_or(WriteError::InvalidInput(
                "quote Accept has no authorization result",
            ))?;
            if !same_remote_note_host(actor_uri, result_uri)? {
                return Err(WriteError::InvalidInput(
                    "quote authorization host does not match its actor",
                ));
            }
        }
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, request_uri).await?;
        if accepted {
            lock_remote_interaction(
                &mut transaction,
                result_uri.expect("accepted quote decision has a result"),
            )
            .await?;
        }
        if !remote_interaction_actor_matches(&mut transaction, source_account_id, actor_uri, true)
            .await?
        {
            transaction.commit().await?;
            return Ok(false);
        }
        let quote = sqlx::query_as::<_, (i64, i64, i64, i64, i32, Option<String>, bool)>(
            "SELECT quote.id, quote.status_id, quote.account_id, quote.quoted_status_id, \
                    quote.state, quote.approval_uri, quote.legacy \
               FROM quotes quote \
               JOIN statuses instrument ON instrument.id = quote.status_id \
               JOIN accounts quoter ON quoter.id = quote.account_id \
              WHERE quote.activity_uri = $1 AND quote.quoted_account_id = $2 \
                AND instrument.local IS TRUE AND instrument.deleted_at IS NULL \
              ORDER BY quote.id LIMIT 1 FOR UPDATE OF quote, instrument",
        )
        .bind(request_uri)
        .bind(source_account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((
            quote_id,
            status_id,
            quoting_account_id,
            quoted_status_id,
            old_state,
            old_approval_uri,
            legacy,
        )) = quote
        else {
            transaction.commit().await?;
            return Ok(false);
        };
        if delivery_target_account_id.is_some_and(|id| id != quoting_account_id) {
            transaction.commit().await?;
            return Ok(true);
        }
        if let Some(request_actor_uri) = request_actor_uri {
            let expected =
                local_actor_uri_for_account(&mut transaction, quoting_account_id, origin).await?;
            if request_actor_uri != expected {
                return Err(WriteError::InvalidInput(
                    "embedded QuoteRequest actor does not match the quote author",
                ));
            }
        }
        if let Some(instrument_uri) = instrument_uri
            && !quote_target_matches_uri(&mut transaction, status_id, instrument_uri, origin)
                .await?
        {
            return Err(WriteError::InvalidInput(
                "embedded QuoteRequest instrument does not match the quote",
            ));
        }
        if let Some(quoted_status_uri) = quoted_status_uri
            && !quote_target_matches_uri(
                &mut transaction,
                quoted_status_id,
                quoted_status_uri,
                origin,
            )
            .await?
        {
            return Err(WriteError::InvalidInput(
                "embedded QuoteRequest object does not match the quote target",
            ));
        }
        if accepted
            && remote_interaction_tombstoned(
                &mut transaction,
                source_account_id,
                result_uri.expect("accepted quote decision has a result"),
            )
            .await?
        {
            let next_state = match old_state {
                0 => 2,
                1 => 3,
                _ => old_state,
            };
            if next_state != old_state || old_approval_uri.is_some() {
                sqlx::query(
                    "UPDATE quotes SET state = $2, approval_uri = NULL, updated_at = clock_timestamp() \
                     WHERE id = $1",
                )
                .bind(quote_id)
                .bind(next_state)
                .execute(&mut *transaction)
                .await?;
                if quote_state_update_counter_delta(legacy, old_state, next_state) < 0 {
                    decrement_quote_count(&mut transaction, quoted_status_id).await?;
                }
                delete_activity_notifications(
                    &mut transaction,
                    source_account_id,
                    quote_id,
                    "Quote",
                )
                .await?;
                let edited_at = record_quote_status_update(&mut transaction, status_id).await?;
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    edited_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            cancel_quote_request_outbox(&mut transaction, quote_id, Some(request_uri)).await?;
            flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
            transaction.commit().await?;
            return Ok(true);
        }
        if accepted && !matches!(old_state, 0 | 1) {
            return Err(WriteError::Conflict);
        }
        let next_state = if accepted {
            if old_state == 0 { 1 } else { old_state }
        } else if matches!(old_state, 1 | 3) {
            3
        } else {
            2
        };
        let next_approval_uri = if accepted && next_state == 1 {
            result_uri.map(ToOwned::to_owned)
        } else {
            None
        };
        if accepted && old_state == 1 && old_approval_uri != next_approval_uri {
            return Err(WriteError::Conflict);
        }
        if next_state != old_state || next_approval_uri != old_approval_uri {
            sqlx::query(
                "UPDATE quotes SET state = $2, approval_uri = $3, updated_at = clock_timestamp() \
                 WHERE id = $1",
            )
            .bind(quote_id)
            .bind(next_state)
            .bind(&next_approval_uri)
            .execute(&mut *transaction)
            .await?;
            if old_state != 1 && next_state == 1 {
                increment_quote_count(&mut transaction, quoted_status_id).await?;
            } else if old_state == 1 && next_state != 1 {
                decrement_quote_count(&mut transaction, quoted_status_id).await?;
            }
            if next_state == 1 {
                let quoted_account_local = sqlx::query_scalar::<_, bool>(
                    "SELECT domain IS NULL FROM accounts WHERE id = $1",
                )
                .bind(source_account_id)
                .fetch_one(&mut *transaction)
                .await?;
                if quoted_account_local {
                    record_outbox_in(
                        &mut transaction,
                        &notification_job(source_account_id, NOTIFICATION_QUOTE, quote_id),
                    )
                    .await?;
                }
            } else {
                delete_activity_notifications(
                    &mut transaction,
                    source_account_id,
                    quote_id,
                    "Quote",
                )
                .await?;
            }
            let edited_at = record_quote_status_update(&mut transaction, status_id).await?;
            collect_status_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                status_id,
                "status.update",
                edited_at.and_utc().timestamp_micros(),
            )
            .await?;
        }
        cancel_quote_request_outbox(&mut transaction, quote_id, Some(request_uri)).await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(true)
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_quote_authorization(
        &self,
        quoting_account_id: i64,
        quoting_uri: &str,
        approval_uri: &str,
        document: &Value,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let authorization = remote_quote_authorization_data(document)?;
        if authorization.uri != approval_uri || !authorization.typed {
            return Err(WriteError::InvalidInput(
                "remote QuoteAuthorization identity or type is invalid",
            ));
        }
        let attributed_to =
            authorization
                .attributed_to
                .as_deref()
                .ok_or(WriteError::InvalidInput(
                    "remote QuoteAuthorization has no attributed actor",
                ))?;
        if !same_remote_note_host(attributed_to, approval_uri)? {
            return Err(WriteError::InvalidInput(
                "remote QuoteAuthorization host does not match its actor",
            ));
        }
        let interacting_object =
            authorization
                .interacting_object
                .as_deref()
                .ok_or(WriteError::InvalidInput(
                    "remote QuoteAuthorization has no interacting object",
                ))?;
        let interaction_target =
            authorization
                .interaction_target
                .as_deref()
                .ok_or(WriteError::InvalidInput(
                    "remote QuoteAuthorization has no interaction target",
                ))?;
        let mut transaction = self.pool.begin().await?;
        lock_remote_interaction(&mut transaction, approval_uri).await?;
        let status_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses \
             WHERE account_id = $2 AND deleted_at IS NULL AND (uri = $1 OR url = $1) \
             ORDER BY id LIMIT 1",
        )
        .bind(quoting_uri)
        .bind(quoting_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let quoted_status_id = sqlx::query_scalar::<_, i64>(
            "SELECT quoted_status_id FROM quotes \
             WHERE status_id = $1 AND quoted_status_id IS NOT NULL",
        )
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(vec![status_id, quoted_status_id])
        .fetch_all(&mut *transaction)
        .await?;
        let (
            quote_id,
            state,
            old_approval_uri,
            quoted_account_id,
            quoted_actor_uri,
            legacy,
            request_uri,
        ) = sqlx::query_as::<_, (i64, i32, Option<String>, i64, String, bool, Option<String>)>(
            "SELECT quote.id, quote.state, quote.approval_uri, quote.quoted_account_id, \
                        quoted_account.uri, quote.legacy, quote.activity_uri \
                   FROM quotes quote \
                   JOIN statuses quoting ON quoting.id = quote.status_id \
                   JOIN statuses quoted ON quoted.id = quote.quoted_status_id \
                   JOIN accounts quoted_account ON quoted_account.id = quote.quoted_account_id \
                  WHERE quote.status_id = $1 AND quote.quoted_status_id = $2 \
                    AND quoting.deleted_at IS NULL AND quoted.deleted_at IS NULL \
                    AND quoted_account.domain IS NOT NULL \
                  FOR UPDATE OF quote",
        )
        .bind(status_id)
        .bind(quoted_status_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if attributed_to != quoted_actor_uri
            || !quote_target_matches_uri(&mut transaction, status_id, interacting_object, origin)
                .await?
            || !quote_target_matches_uri(
                &mut transaction,
                quoted_status_id,
                interaction_target,
                origin,
            )
            .await?
        {
            return Err(WriteError::InvalidInput(
                "remote QuoteAuthorization does not match its quote",
            ));
        }
        if remote_interaction_tombstoned(&mut transaction, quoted_account_id, approval_uri).await? {
            transaction.commit().await?;
            return Ok(false);
        }
        cancel_quote_request_outbox(&mut transaction, quote_id, request_uri.as_deref()).await?;
        if state == 1 {
            let replay = old_approval_uri.as_deref() == Some(approval_uri);
            transaction.commit().await?;
            return Ok(replay);
        }
        if state != 0 {
            transaction.commit().await?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE quotes SET state = 1, approval_uri = $2, updated_at = clock_timestamp() \
             WHERE id = $1 AND state = 0",
        )
        .bind(quote_id)
        .bind(approval_uri)
        .execute(&mut *transaction)
        .await?;
        if quote_state_update_counter_delta(legacy, state, 1) > 0 {
            increment_quote_count(&mut transaction, quoted_status_id).await?;
        }
        if sqlx::query_scalar::<_, bool>("SELECT domain IS NULL FROM accounts WHERE id = $1")
            .bind(quoted_account_id)
            .fetch_one(&mut *transaction)
            .await?
        {
            record_outbox_in(
                &mut transaction,
                &notification_job(quoted_account_id, NOTIFICATION_QUOTE, quote_id),
            )
            .await?;
        }
        let mut pending_stream_events = Vec::new();
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            Utc::now().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub(crate) async fn apply_remote_quote_authorization_delete(
        &self,
        source_account_id: i64,
        actor_uri: &str,
        authorization_uri: &str,
        forwarding_activity: Option<&Value>,
    ) -> Result<bool, WriteError> {
        if !same_remote_note_host(actor_uri, authorization_uri)? {
            return Err(WriteError::InvalidInput(
                "QuoteAuthorization Delete host does not match its actor",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, authorization_uri).await?;
        if !remote_interaction_actor_matches(&mut transaction, source_account_id, actor_uri, false)
            .await?
        {
            transaction.commit().await?;
            return Ok(false);
        }
        insert_remote_note_tombstone(&mut transaction, source_account_id, authorization_uri)
            .await?;
        let quote = sqlx::query_as::<_, (i64, i64, i64, i32, bool, Option<String>)>(
            "SELECT quote.id, quote.status_id, quote.quoted_status_id, quote.state, quote.legacy, \
                    quote.activity_uri \
               FROM quotes quote \
               JOIN statuses status ON status.id = quote.status_id \
              WHERE quote.approval_uri = $1 AND quote.quoted_account_id = $2 \
                AND quote.state IN (0, 1) AND status.deleted_at IS NULL \
              ORDER BY quote.id LIMIT 1 FOR UPDATE OF quote, status",
        )
        .bind(authorization_uri)
        .bind(source_account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((quote_id, status_id, quoted_status_id, old_state, legacy, request_uri)) = quote
        else {
            transaction.commit().await?;
            return Ok(false);
        };
        cancel_quote_request_outbox(&mut transaction, quote_id, request_uri.as_deref()).await?;
        if let Some(activity) = forwarding_activity {
            record_remote_quote_authorization_forwarding_in(
                &mut transaction,
                source_account_id,
                actor_uri,
                status_id,
                activity,
            )
            .await?;
        }
        let next_state = if old_state == 1 { 3 } else { 2 };
        sqlx::query(
            "UPDATE quotes SET state = $2, approval_uri = NULL, updated_at = clock_timestamp() \
             WHERE id = $1",
        )
        .bind(quote_id)
        .bind(next_state)
        .execute(&mut *transaction)
        .await?;
        if quote_state_update_counter_delta(legacy, old_state, next_state) < 0 {
            decrement_quote_count(&mut transaction, quoted_status_id).await?;
        }
        delete_activity_notifications(&mut transaction, source_account_id, quote_id, "Quote")
            .await?;
        let status_is_local = sqlx::query_scalar::<_, bool>(
            "SELECT account.domain IS NULL FROM statuses status \
             JOIN accounts account ON account.id = status.account_id WHERE status.id = $1",
        )
        .bind(status_id)
        .fetch_one(&mut *transaction)
        .await?;
        let updated_at = if status_is_local {
            record_quote_status_update(&mut transaction, status_id).await?
        } else {
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?
        };
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            updated_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn remote_quote_request_may_import(
        &self,
        source_account_id: i64,
        request_uri: &str,
        actor_uri: &str,
        quoted_status_uri: &str,
        instrument_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<Option<(i64, i64)>, WriteError> {
        if !same_remote_note_host(actor_uri, request_uri)?
            || !same_remote_note_host(actor_uri, instrument_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote QuoteRequest identifiers do not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        lock_remote_interaction(&mut transaction, request_uri).await?;
        if !remote_interaction_actor_matches(&mut transaction, source_account_id, actor_uri, true)
            .await?
        {
            transaction.commit().await?;
            return Ok(None);
        }
        if remote_quote_request_decision_in(
            &mut transaction,
            request_uri,
            actor_uri,
            quoted_status_uri,
            instrument_uri,
        )
        .await?
        .is_some()
        {
            transaction.commit().await?;
            return Ok(None);
        }
        let Some((target_status_id, target_account_id, target_local, _)) =
            resolve_quote_target(&mut transaction, quoted_status_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        if !target_local || delivery_target_account_id.is_some_and(|id| id != target_account_id) {
            transaction.commit().await?;
            return Ok(None);
        }
        match writable_quote_target(&mut transaction, source_account_id, target_status_id).await {
            Ok(_) => {
                transaction.commit().await?;
                Ok(Some((target_status_id, target_account_id)))
            }
            Err(WriteError::NotFound | WriteError::Forbidden) => {
                transaction.commit().await?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn apply_remote_quote_request(
        &self,
        source_account_id: i64,
        request_uri: &str,
        actor_uri: &str,
        quoted_status_uri: &str,
        instrument_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if !same_remote_note_host(actor_uri, request_uri)?
            || !same_remote_note_host(actor_uri, instrument_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote QuoteRequest identifiers do not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, request_uri).await?;
        if !remote_interaction_actor_matches(&mut transaction, source_account_id, actor_uri, true)
            .await?
        {
            transaction.commit().await?;
            return Ok(());
        }
        if remote_quote_request_decision_in(
            &mut transaction,
            request_uri,
            actor_uri,
            quoted_status_uri,
            instrument_uri,
        )
        .await?
        .is_some()
        {
            transaction.commit().await?;
            return Ok(());
        }
        let Some((target_status_id, target_account_id, target_local, target_actor_uri)) =
            resolve_quote_target(&mut transaction, quoted_status_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        if !target_local || delivery_target_account_id.is_some_and(|id| id != target_account_id) {
            transaction.commit().await?;
            return Ok(());
        }
        let decision_allowed =
            writable_quote_target(&mut transaction, source_account_id, target_status_id)
                .await
                .is_ok();
        let source_delivery = sqlx::query_as::<_, (String, String)>(
            "SELECT inbox_url, domain FROM accounts \
             WHERE id = $1 AND domain IS NOT NULL AND uri = $2 AND protocol = 1 \
               AND suspended_at IS NULL FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(actor_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((inbox_url, remote_domain)) = source_delivery else {
            transaction.commit().await?;
            return Ok(());
        };
        let quote = sqlx::query_as::<_, (i64, i64, i32, Option<String>, bool)>(
            "SELECT quote.id, quote.status_id, quote.state, quote.activity_uri, quote.legacy \
               FROM quotes quote \
               JOIN statuses instrument ON instrument.id = quote.status_id \
              WHERE instrument.account_id = $1 AND instrument.deleted_at IS NULL \
                AND (instrument.uri = $2 OR instrument.url = $2) \
                AND quote.quoted_status_id = $3 \
              ORDER BY quote.id LIMIT 1 FOR UPDATE OF quote",
        )
        .bind(source_account_id)
        .bind(instrument_uri)
        .bind(target_status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let decision_quote_identity = quote
            .as_ref()
            .map(|(quote_id, quoting_status_id, _, _, _)| (*quote_id, *quoting_status_id));
        let mut accepted = false;
        if let Some((quote_id, quoting_status_id, state, activity_uri, legacy)) = quote.as_ref() {
            if activity_uri
                .as_deref()
                .is_some_and(|uri| uri != request_uri)
            {
                return Err(WriteError::Conflict);
            }
            if decision_allowed && matches!(*state, 0 | 1) {
                accepted = true;
                if *state == 0 {
                    sqlx::query(
                        "UPDATE quotes SET state = 1, activity_uri = $2, approval_uri = NULL, \
                                updated_at = clock_timestamp() WHERE id = $1 AND state = 0",
                    )
                    .bind(quote_id)
                    .bind(request_uri)
                    .execute(&mut *transaction)
                    .await?;
                    if quote_state_update_counter_delta(*legacy, *state, 1) > 0 {
                        increment_quote_count(&mut transaction, target_status_id).await?;
                    }
                    record_outbox_in(
                        &mut transaction,
                        &notification_job(target_account_id, NOTIFICATION_QUOTE, *quote_id),
                    )
                    .await?;
                    collect_status_stream_events(
                        &mut transaction,
                        &mut pending_stream_events,
                        *quoting_status_id,
                        "status.update",
                        Utc::now().timestamp_micros(),
                    )
                    .await?;
                } else if activity_uri.is_none() {
                    sqlx::query(
                        "UPDATE quotes SET activity_uri = $2, updated_at = clock_timestamp() \
                         WHERE id = $1 AND activity_uri IS NULL",
                    )
                    .bind(quote_id)
                    .bind(request_uri)
                    .execute(&mut *transaction)
                    .await?;
                }
            } else if !decision_allowed && matches!(*state, 0 | 1) {
                let next_state = if *state == 1 { 3 } else { 2 };
                sqlx::query(
                    "UPDATE quotes SET state = $2, activity_uri = $3, approval_uri = NULL, \
                            updated_at = clock_timestamp() WHERE id = $1",
                )
                .bind(quote_id)
                .bind(next_state)
                .bind(request_uri)
                .execute(&mut *transaction)
                .await?;
                if quote_state_update_counter_delta(*legacy, *state, next_state) < 0 {
                    decrement_quote_count(&mut transaction, target_status_id).await?;
                }
                delete_activity_notifications(
                    &mut transaction,
                    target_account_id,
                    *quote_id,
                    "Quote",
                )
                .await?;
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    *quoting_status_id,
                    "status.update",
                    Utc::now().timestamp_micros(),
                )
                .await?;
            }
        }
        let quote_id = if accepted {
            decision_quote_identity
                .map(|(quote_id, _)| quote_id)
                .expect("accepted QuoteRequest has a persisted quote")
        } else {
            activitypub::quote_request_rejection_id(request_uri)
        };
        let authorization_uri = accepted.then(|| {
            format!(
                "{}/quote_authorizations/{quote_id}",
                target_actor_uri.trim_end_matches('/')
            )
        });
        let body = activitypub::quote_decision_with_uris(
            &target_actor_uri,
            quote_id,
            request_uri,
            actor_uri,
            quoted_status_uri,
            instrument_uri,
            authorization_uri.as_deref(),
            accepted,
        );
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": target_account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": body,
                "quote_delivery_kind": if accepted { "accept" } else { "reject" },
                "quote_request_uri": request_uri,
                "quote_id": decision_quote_identity.map(|(quote_id, _)| quote_id),
                "quoting_status_id": decision_quote_identity.map(|(_, status_id)| status_id),
                "quoted_status_id": target_status_id
            }),
        )
        .logical_key(activitypub::quote_request_decision_logical_key(request_uri));
        if !record_outbox_once_in(&mut transaction, &delivery).await? {
            return Err(WriteError::Conflict);
        }
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_follow(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        object_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<Option<RemoteFollowOutcome>, WriteError> {
        if follow_uri.trim().is_empty() || object_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Follow is missing its activity or object URI",
            ));
        }
        let preheal_target = self
            .local_activitypub_account_id_before_relationship_locks(object_uri, origin)
            .await?;
        if let Some(target_account_id) = preheal_target.filter(|id| *id != -99)
            && delivery_target_account_id.is_none_or(|id| id == -99 || id == target_account_id)
        {
            self.preheal_relationship_account_stats(source_account_id, target_account_id)
                .await?;
        }
        let tombstone_key = follow_tombstone_key(source_account_id, follow_uri);
        let mut transaction = self.pool.begin().await?;
        lock_follow_tombstone(&mut transaction, &tombstone_key).await?;
        let tombstoned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM rustodon.idempotency_keys
                  WHERE scope = $1 AND key = $2 AND expires_at > clock_timestamp())",
        )
        .bind(RELATIONSHIP_TOMBSTONE_SCOPE)
        .bind(&tombstone_key)
        .fetch_one(&mut *transaction)
        .await?;
        if tombstoned {
            transaction.commit().await?;
            return Ok(None);
        }
        if remote_interaction_tombstoned(&mut transaction, source_account_id, follow_uri).await? {
            transaction.commit().await?;
            return Ok(None);
        }

        let Some(target_account_id) =
            local_activitypub_account_id(&mut transaction, object_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        if delivery_target_account_id.is_some_and(|id| id != -99 && id != target_account_id) {
            transaction.commit().await?;
            return Ok(None);
        }
        if target_account_id == -99 {
            transaction.commit().await?;
            return Ok(Some(RemoteFollowOutcome::Rejected {
                recipient_account_id: target_account_id,
            }));
        }
        lock_relationship(&mut transaction, source_account_id, target_account_id).await?;
        let Some((
            target_locked,
            target_unavailable,
            source_suspended,
            source_silenced,
            domain_blocked,
        )) = sqlx::query_as::<_, (bool, bool, bool, bool, bool)>(
            "SELECT target.locked,
                        target.suspended_at IS NOT NULL OR target.moved_to_account_id IS NOT NULL,
                        source.suspended_at IS NOT NULL,
                        source.silenced_at IS NOT NULL,
                        EXISTS (
                          SELECT 1 FROM account_domain_blocks domain_block
                           WHERE domain_block.account_id = target.id
                             AND domain_block.domain = source.domain)
                   FROM accounts source
                   JOIN accounts target ON target.id = $2
                  WHERE source.id = $1 AND source.domain IS NOT NULL
                  FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        if source_suspended {
            transaction.commit().await?;
            return Ok(None);
        }

        if let Some(existing_id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM follow_requests
               WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            update_follow_uri(
                &mut transaction,
                "follow_requests",
                source_account_id,
                target_account_id,
                follow_uri,
            )
            .await?;
            ensure_relationship_account_stats(
                &mut transaction,
                source_account_id,
                target_account_id,
            )
            .await?;
            transaction.commit().await?;
            return Ok(Some(RemoteFollowOutcome::Applied(
                RemoteFollowWriteOutcome {
                    activity_id: existing_id,
                    recipient_account_id: target_account_id,
                    request: true,
                    created: false,
                    silenced: source_silenced,
                },
            )));
        }

        if target_unavailable
            || domain_blocked
            || relationship_is_blocked(&mut transaction, source_account_id, target_account_id)
                .await?
        {
            transaction.commit().await?;
            return Ok(Some(RemoteFollowOutcome::Rejected {
                recipient_account_id: target_account_id,
            }));
        }

        if let Some(existing_id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM follows
               WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            update_follow_uri(
                &mut transaction,
                "follows",
                source_account_id,
                target_account_id,
                follow_uri,
            )
            .await?;
            ensure_relationship_account_stats(
                &mut transaction,
                source_account_id,
                target_account_id,
            )
            .await?;
            transaction.commit().await?;
            return Ok(Some(RemoteFollowOutcome::Applied(
                RemoteFollowWriteOutcome {
                    activity_id: existing_id,
                    recipient_account_id: target_account_id,
                    request: false,
                    created: false,
                    silenced: source_silenced,
                },
            )));
        }

        let request = target_locked || source_silenced;
        let table = if request {
            "follow_requests"
        } else {
            "follows"
        };

        let activity_id = sqlx::query_scalar::<_, i64>(&format!(
            "INSERT INTO {table} (
                 account_id, target_account_id, show_reblogs, notify, languages, uri,
                 created_at, updated_at)
             VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())
             RETURNING id"
        ))
        .bind(source_account_id)
        .bind(target_account_id)
        .bind(follow_uri)
        .fetch_one(&mut *transaction)
        .await?;
        if !request {
            increment_follow_counts(&mut transaction, source_account_id, target_account_id).await?;
        }
        let activity_type = if request {
            NOTIFICATION_FOLLOW_REQUEST
        } else {
            NOTIFICATION_FOLLOW
        };
        record_outbox_in(
            &mut transaction,
            &notification_job_with_silenced(
                target_account_id,
                activity_type,
                activity_id,
                source_silenced,
            ),
        )
        .await?;
        ensure_relationship_account_stats(&mut transaction, source_account_id, target_account_id)
            .await?;
        transaction.commit().await?;
        Ok(Some(RemoteFollowOutcome::Applied(
            RemoteFollowWriteOutcome {
                activity_id,
                recipient_account_id: target_account_id,
                request,
                created: true,
                silenced: source_silenced,
            },
        )))
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_undo_follow(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        target_uri: Option<&str>,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if follow_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Undo object is missing its URI",
            ));
        }
        let preheal_target = if let Some(target_uri) = target_uri {
            self.local_activitypub_account_id_before_relationship_locks(target_uri, origin)
                .await?
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT target_account_id FROM follows \
                  WHERE account_id = $1 AND uri = $2 \
                 UNION ALL \
                SELECT target_account_id FROM follow_requests \
                  WHERE account_id = $1 AND uri = $2 \
                 LIMIT 1",
            )
            .bind(source_account_id)
            .bind(follow_uri)
            .fetch_optional(&self.pool)
            .await?
        };
        if let Some(target_account_id) = preheal_target.filter(|id| *id != -99)
            && delivery_target_account_id.is_none_or(|id| id == -99 || id == target_account_id)
        {
            self.preheal_relationship_account_stats(source_account_id, target_account_id)
                .await?;
        }
        let tombstone_key = follow_tombstone_key(source_account_id, follow_uri);
        let mut transaction = self.pool.begin().await?;
        lock_follow_tombstone(&mut transaction, &tombstone_key).await?;
        let source_is_remote =
            sqlx::query_scalar::<_, bool>("SELECT domain IS NOT NULL FROM accounts WHERE id = $1")
                .bind(source_account_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false);
        if !source_is_remote {
            transaction.commit().await?;
            return Ok(());
        }

        let target_from_object = match target_uri {
            Some(target_uri) => {
                let Some(target_account_id) =
                    local_activitypub_account_id(&mut transaction, target_uri, origin).await?
                else {
                    transaction.commit().await?;
                    return Ok(());
                };
                Some(target_account_id)
            }
            None => None,
        };
        let target_account_id = if target_from_object.is_some() {
            target_from_object
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT target_account_id FROM follows
                  WHERE account_id = $1 AND uri = $2
                 UNION ALL
                SELECT target_account_id FROM follow_requests
                  WHERE account_id = $1 AND uri = $2
                 LIMIT 1",
            )
            .bind(source_account_id)
            .bind(follow_uri)
            .fetch_optional(&mut *transaction)
            .await?
        };

        if delivery_target_account_id
            .is_some_and(|id| id != -99 && target_account_id.is_some_and(|target| target != id))
        {
            transaction.commit().await?;
            return Ok(());
        }

        if let Some(target_account_id) = target_account_id.filter(|id| *id != -99) {
            lock_relationship(&mut transaction, source_account_id, target_account_id).await?;
            let removed_follow = sqlx::query_scalar::<_, i64>(
                "DELETE FROM follows
                  WHERE account_id = $1 AND target_account_id = $2 AND uri = $3
                  RETURNING id",
            )
            .bind(source_account_id)
            .bind(target_account_id)
            .bind(follow_uri)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some(follow_id) = removed_follow {
                decrement_follow_counts(&mut transaction, source_account_id, target_account_id)
                    .await?;
                delete_activity_notifications(
                    &mut transaction,
                    target_account_id,
                    follow_id,
                    "Follow",
                )
                .await?;
            } else {
                let removed_request = sqlx::query_scalar::<_, i64>(
                    "DELETE FROM follow_requests
                      WHERE account_id = $1 AND target_account_id = $2 AND uri = $3
                      RETURNING id",
                )
                .bind(source_account_id)
                .bind(target_account_id)
                .bind(follow_uri)
                .fetch_optional(&mut *transaction)
                .await?;
                if let Some(request_id) = removed_request {
                    delete_follow_request_notifications(
                        &mut transaction,
                        target_account_id,
                        request_id,
                    )
                    .await?;
                }
            }
            ensure_relationship_account_stats(
                &mut transaction,
                source_account_id,
                target_account_id,
            )
            .await?;
        }
        insert_relationship_tombstone(&mut transaction, &tombstone_key, follow_uri).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_block(
        &self,
        source_account_id: i64,
        block_uri: &str,
        object_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if block_uri.trim().is_empty() || object_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Block is missing its activity or object URI",
            ));
        }
        let preheal_target = self
            .local_activitypub_account_id_before_relationship_locks(object_uri, origin)
            .await?;
        if let Some(target_account_id) = preheal_target.filter(|id| *id != -99)
            && delivery_target_account_id.is_none_or(|id| id == -99 || id == target_account_id)
        {
            self.preheal_relationship_account_stats(source_account_id, target_account_id)
                .await?;
        }
        let tombstone_key = block_tombstone_key(source_account_id, block_uri);
        let mut transaction = self.pool.begin().await?;
        lock_follow_tombstone(&mut transaction, &tombstone_key).await?;
        let tombstoned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM rustodon.idempotency_keys
                  WHERE scope = $1 AND key = $2 AND expires_at > clock_timestamp())",
        )
        .bind(RELATIONSHIP_TOMBSTONE_SCOPE)
        .bind(&tombstone_key)
        .fetch_one(&mut *transaction)
        .await?;
        if tombstoned {
            transaction.commit().await?;
            return Ok(());
        }
        if remote_interaction_tombstoned(&mut transaction, source_account_id, block_uri).await? {
            transaction.commit().await?;
            return Ok(());
        }

        let Some(target_account_id) =
            local_activitypub_account_id(&mut transaction, object_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        if target_account_id == -99
            || delivery_target_account_id.is_some_and(|id| id != -99 && id != target_account_id)
        {
            transaction.commit().await?;
            return Ok(());
        }
        let source_is_remote =
            sqlx::query_scalar::<_, bool>("SELECT domain IS NOT NULL FROM accounts WHERE id = $1")
                .bind(source_account_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false);
        if !source_is_remote {
            transaction.commit().await?;
            return Ok(());
        }

        lock_relationship(&mut transaction, source_account_id, target_account_id).await?;
        sqlx::query(
            "DELETE FROM notification_permissions
              WHERE account_id = $1 AND from_account_id = $2",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .execute(&mut *transaction)
        .await?;
        remove_follow_relationships(&mut transaction, source_account_id, target_account_id).await?;
        sqlx::query(
            "INSERT INTO blocks (account_id, created_at, target_account_id, updated_at, uri)
             VALUES ($1, clock_timestamp(), $2, clock_timestamp(), $3)
             ON CONFLICT (account_id, target_account_id) DO UPDATE SET
               uri = EXCLUDED.uri, updated_at = clock_timestamp()",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .bind(block_uri)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_undo_block(
        &self,
        source_account_id: i64,
        block_uri: &str,
        target_uri: Option<&str>,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if block_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Undo Block is missing its URI",
            ));
        }
        let tombstone_key = block_tombstone_key(source_account_id, block_uri);
        let mut transaction = self.pool.begin().await?;
        lock_follow_tombstone(&mut transaction, &tombstone_key).await?;
        let source_is_remote =
            sqlx::query_scalar::<_, bool>("SELECT domain IS NOT NULL FROM accounts WHERE id = $1")
                .bind(source_account_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false);
        if !source_is_remote {
            transaction.commit().await?;
            return Ok(());
        }

        let target_from_object = match target_uri {
            Some(target_uri) => {
                let Some(target_account_id) =
                    local_activitypub_account_id(&mut transaction, target_uri, origin).await?
                else {
                    transaction.commit().await?;
                    return Ok(());
                };
                Some(target_account_id)
            }
            None => None,
        };
        let target_account_id = if target_from_object.is_some() {
            target_from_object
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT target_account_id FROM blocks
                  WHERE account_id = $1 AND uri = $2
                  LIMIT 1",
            )
            .bind(source_account_id)
            .bind(block_uri)
            .fetch_optional(&mut *transaction)
            .await?
        };
        if delivery_target_account_id
            .is_some_and(|id| id != -99 && target_account_id.is_some_and(|target| target != id))
        {
            transaction.commit().await?;
            return Ok(());
        }

        if let Some(target_account_id) = target_account_id.filter(|id| *id != -99) {
            lock_relationship(&mut transaction, source_account_id, target_account_id).await?;
            sqlx::query(
                "DELETE FROM blocks
                  WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
            )
            .bind(source_account_id)
            .bind(target_account_id)
            .bind(block_uri)
            .execute(&mut *transaction)
            .await?;
        }
        insert_relationship_tombstone(&mut transaction, &tombstone_key, block_uri).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn apply_remote_follow_decision(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        target_uri: Option<&str>,
        local_actor_uri: Option<&str>,
        accepted: bool,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if follow_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Follow decision is missing its Follow URI",
            ));
        }
        let preheal_local_account_id = if let Some(local_actor_uri) = local_actor_uri {
            self.local_activitypub_account_id_before_relationship_locks(local_actor_uri, origin)
                .await?
        } else if let Some(delivery_target_account_id) =
            delivery_target_account_id.filter(|id| *id != -99)
        {
            Some(delivery_target_account_id)
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT account_id FROM follows \
                  WHERE target_account_id = $1 AND uri = $2 \
                 UNION ALL \
                SELECT account_id FROM follow_requests \
                  WHERE target_account_id = $1 AND uri = $2 \
                 LIMIT 1",
            )
            .bind(source_account_id)
            .bind(follow_uri)
            .fetch_optional(&self.pool)
            .await?
        };
        if let Some(local_account_id) = preheal_local_account_id.filter(|id| *id != -99)
            && delivery_target_account_id.is_none_or(|id| id == -99 || id == local_account_id)
        {
            self.preheal_relationship_account_stats(local_account_id, source_account_id)
                .await?;
        }
        let mut transaction = self.pool.begin().await?;
        let source_uri = sqlx::query_scalar::<_, String>(
            "SELECT uri FROM accounts WHERE id = $1 AND domain IS NOT NULL",
        )
        .bind(source_account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(source_uri) = source_uri else {
            transaction.commit().await?;
            return Ok(());
        };
        if target_uri.is_some_and(|target_uri| target_uri != source_uri) {
            transaction.commit().await?;
            return Ok(());
        }

        let local_account_id = if let Some(local_actor_uri) = local_actor_uri {
            let local_account_id =
                local_activitypub_account_id(&mut transaction, local_actor_uri, origin).await?;
            if local_account_id.is_none() {
                transaction.commit().await?;
                return Ok(());
            }
            local_account_id
        } else if let Some(delivery_target_account_id) =
            delivery_target_account_id.filter(|id| *id != -99)
        {
            Some(delivery_target_account_id)
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT account_id FROM follows
                  WHERE target_account_id = $1 AND uri = $2
                 UNION ALL
                SELECT account_id FROM follow_requests
                  WHERE target_account_id = $1 AND uri = $2
                 LIMIT 1",
            )
            .bind(source_account_id)
            .bind(follow_uri)
            .fetch_optional(&mut *transaction)
            .await?
        };
        let Some(local_account_id) = local_account_id.filter(|id| *id != -99) else {
            transaction.commit().await?;
            return Ok(());
        };
        if delivery_target_account_id.is_some_and(|id| id != -99 && id != local_account_id) {
            transaction.commit().await?;
            return Ok(());
        }
        lock_relationship(&mut transaction, local_account_id, source_account_id).await?;

        let request = sqlx::query_as::<_, (i64, bool, bool, Option<Vec<String>>, Option<String>)>(
            "SELECT id, show_reblogs, notify, languages, uri
               FROM follow_requests
               WHERE account_id = $1 AND target_account_id = $2
                 AND uri = $3
               FOR UPDATE",
        )
        .bind(local_account_id)
        .bind(source_account_id)
        .bind(follow_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        let follow_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM follows
              WHERE account_id = $1 AND target_account_id = $2
                AND uri = $3
              FOR UPDATE",
        )
        .bind(local_account_id)
        .bind(source_account_id)
        .bind(follow_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        if accepted {
            if let Some(request) = request {
                if follow_id.is_none() {
                    sqlx::query(
                        "INSERT INTO follows (
                           account_id, target_account_id, show_reblogs, notify, languages, uri,
                           created_at, updated_at)
                         VALUES ($1, $2, $3, $4, $5, $6, clock_timestamp(), clock_timestamp())",
                    )
                    .bind(local_account_id)
                    .bind(source_account_id)
                    .bind(request.1)
                    .bind(request.2)
                    .bind(request.3)
                    .bind(follow_uri)
                    .execute(&mut *transaction)
                    .await?;
                    increment_follow_counts(&mut transaction, local_account_id, source_account_id)
                        .await?;
                } else {
                    sqlx::query(
                        "UPDATE follows SET uri = $3, updated_at = clock_timestamp()
                          WHERE account_id = $1 AND target_account_id = $2",
                    )
                    .bind(local_account_id)
                    .bind(source_account_id)
                    .bind(follow_uri)
                    .execute(&mut *transaction)
                    .await?;
                }
                sqlx::query(
                    "DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2",
                )
                .bind(local_account_id)
                .bind(source_account_id)
                .execute(&mut *transaction)
                .await?;
                delete_follow_request_notifications(&mut transaction, local_account_id, request.0)
                    .await?;
            } else if follow_id.is_some() {
                sqlx::query(
                    "UPDATE follows SET uri = $3, updated_at = clock_timestamp()
                      WHERE account_id = $1 AND target_account_id = $2",
                )
                .bind(local_account_id)
                .bind(source_account_id)
                .bind(follow_uri)
                .execute(&mut *transaction)
                .await?;
            }
        } else {
            if let Some(request) = request {
                sqlx::query(
                    "DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2",
                )
                .bind(local_account_id)
                .bind(source_account_id)
                .execute(&mut *transaction)
                .await?;
                delete_follow_request_notifications(&mut transaction, local_account_id, request.0)
                    .await?;
            }
            if let Some(follow_id) = follow_id {
                sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
                    .bind(local_account_id)
                    .bind(source_account_id)
                    .execute(&mut *transaction)
                    .await?;
                decrement_follow_counts(&mut transaction, local_account_id, source_account_id)
                    .await?;
                delete_activity_notifications(
                    &mut transaction,
                    source_account_id,
                    follow_id,
                    "Follow",
                )
                .await?;
            }
        }
        ensure_relationship_account_stats(&mut transaction, local_account_id, source_account_id)
            .await?;
        transaction.commit().await?;
        Ok(())
    }
}

#[allow(clippy::missing_errors_doc)]
impl WriteRepository {
    /// Read-only preflight for user-initiated resolution. This does not confer inbox
    /// delivery authority: only the document's existing audience can admit a viewer.
    pub(crate) async fn remote_note_search_allowed(
        &self,
        viewer: i64,
        actor_uri: &str,
        object: &Value,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let note = RemoteNoteData::parse(object, actor_uri)?;
        if !same_remote_note_host(actor_uri, &note.uri)? {
            return Ok(false);
        }
        let mut transaction = self.pool.begin().await?;
        let denied: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM accounts author WHERE author.uri=$1 AND (
                author.domain IS NULL OR author.suspended_at IS NOT NULL
                OR EXISTS (SELECT 1 FROM blocks WHERE (account_id=$2 AND target_account_id=author.id)
                    OR (account_id=author.id AND target_account_id=$2))
                OR EXISTS (SELECT 1 FROM mutes WHERE account_id=$2 AND target_account_id=author.id)
                OR EXISTS (SELECT 1 FROM account_domain_blocks WHERE account_id=$2 AND domain=author.domain)))
             OR EXISTS (SELECT 1 FROM tombstones WHERE uri=$3 OR uri=$4)
             OR EXISTS (SELECT 1 FROM statuses status JOIN accounts author ON author.id=status.account_id
                 WHERE (status.uri=$3 OR status.uri=$4) AND author.uri<>$1)",
        ).bind(actor_uri).bind(viewer).bind(&note.uri).bind(&note.atom_uri)
            .fetch_one(&mut *transaction).await?;
        if denied {
            return Ok(false);
        }
        let author: Option<(i64, String)> = sqlx::query_as(
            "SELECT id, followers_url FROM accounts WHERE uri=$1 AND domain IS NOT NULL",
        )
        .bind(actor_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        let visibility = remote_note_visibility(
            &note.audience,
            author
                .as_ref()
                .map_or("", |(_, followers)| followers.as_str()),
        );
        if matches!(visibility, 0 | 1) {
            return Ok(true);
        }
        for uri in note
            .audience
            .to
            .iter()
            .chain(&note.audience.cc)
            .chain(&note.mentions)
        {
            if local_activitypub_account_id(&mut transaction, uri, origin).await? == Some(viewer) {
                return Ok(true);
            }
        }
        if visibility == 2
            && let Some((author, _)) = author
        {
            return Ok(sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM follows WHERE account_id=$1 AND target_account_id=$2)",
            ).bind(viewer).bind(author).fetch_one(&mut *transaction).await?);
        }
        Ok(false)
    }

    pub(crate) async fn remote_note_is_relevant(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let note = RemoteNoteData::parse(object, actor_uri)?;
        if delivery_target_account_id.is_some() {
            return Ok(true);
        }
        let mut transaction = self.pool.begin().await?;
        let followers_url = sqlx::query_scalar::<_, String>(
            "SELECT followers_url FROM accounts WHERE id = $1 AND domain IS NOT NULL",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let addressed = {
            let audience = note.audience.to.iter().chain(&note.audience.cc);
            let mut addressed = false;
            for uri in audience {
                if local_activitypub_account_id(&mut transaction, uri, origin)
                    .await?
                    .is_some()
                {
                    addressed = true;
                    break;
                }
            }
            addressed
        };
        let followed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM follows follow
                JOIN accounts local_account ON local_account.id = follow.account_id
                                             AND local_account.domain IS NULL
                WHERE follow.target_account_id = $1)",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        let (_, parent_account_id, _) = remote_note_thread(&mut transaction, &note, origin).await?;
        let parent_relevant = if let Some(parent_account_id) = parent_account_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                    SELECT 1 FROM accounts parent
                    WHERE parent.id = $1 AND parent.domain IS NULL
                ) OR EXISTS (
                    SELECT 1 FROM follows follow
                    JOIN accounts local_account ON local_account.id = follow.account_id
                                                 AND local_account.domain IS NULL
                    WHERE follow.target_account_id = $1)",
            )
            .bind(parent_account_id)
            .fetch_one(&mut *transaction)
            .await?
        } else {
            false
        };
        let visibility = remote_note_visibility(&note.audience, &followers_url);
        let relevant = match visibility {
            0 | 1 => addressed || followed || parent_relevant,
            2 => addressed || followed,
            _ => addressed,
        };
        transaction.commit().await?;
        Ok(relevant)
    }

    pub(crate) async fn remote_quote_target_is_local(
        &self,
        target_uri: &str,
        origin: &str,
    ) -> Result<Option<bool>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let target = resolve_quote_target(&mut transaction, target_uri, origin)
            .await?
            .map(|(_, _, local, _)| local);
        transaction.commit().await?;
        Ok(target)
    }

    pub(crate) async fn remote_domain_allowed_in_transaction(
        transaction: &mut Transaction<'_, Postgres>,
        domain: &str,
        limited_federation: bool,
    ) -> Result<bool, WriteError> {
        remote_domain_allowed_in_transaction(transaction, domain, limited_federation).await
    }

    pub(crate) async fn remote_quote_target_matches_status(
        &self,
        status_id: i64,
        target_uri: &str,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let matches =
            quote_target_matches_uri(&mut transaction, status_id, target_uri, origin).await?;
        transaction.commit().await?;
        Ok(matches)
    }

    pub(crate) async fn remote_note_exists(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
    ) -> Result<bool, WriteError> {
        let note = RemoteNoteData::parse(object, actor_uri)?;
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM statuses
                WHERE account_id = $1
                  AND (uri = $2 OR ($3::text IS NOT NULL AND uri = $3)))",
        )
        .bind(account_id)
        .bind(&note.uri)
        .bind(&note.atom_uri)
        .fetch_one(&self.pool)
        .await?)
    }

    pub(crate) async fn remote_note_reference_is_resolved(
        &self,
        account_id: i64,
        actor_uri: &str,
        object_uri: &str,
    ) -> Result<bool, WriteError> {
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Note URI does not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        lock_remote_note(&mut transaction, object_uri).await?;
        let actor_matches = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM accounts
                WHERE id = $1 AND domain IS NOT NULL AND uri = $2)",
        )
        .bind(account_id)
        .bind(actor_uri)
        .fetch_one(&mut *transaction)
        .await?;
        if !actor_matches {
            transaction.commit().await?;
            return Ok(true);
        }
        let resolved = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM statuses WHERE account_id = $1 AND uri = $2
                UNION ALL
                SELECT 1 FROM tombstones WHERE account_id = $1 AND uri = $2)",
        )
        .bind(account_id)
        .bind(object_uri)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(resolved)
    }

    pub(crate) async fn ensure_remote_note_reference_delivery(
        &self,
        account_id: i64,
        actor_uri: &str,
        object_uri: &str,
        delivery_target_account_id: i64,
    ) -> Result<(), WriteError> {
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Note URI does not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        lock_remote_note(&mut transaction, object_uri).await?;
        let status_id = sqlx::query_scalar::<_, i64>(
            "SELECT status.id FROM statuses status
               JOIN accounts actor ON actor.id = status.account_id
                                  AND actor.id = $1 AND actor.uri = $2
                                  AND actor.domain IS NOT NULL
              WHERE status.uri = $3 AND status.deleted_at IS NULL
              ORDER BY status.id LIMIT 1",
        )
        .bind(account_id)
        .bind(actor_uri)
        .bind(object_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(status_id) = status_id {
            ensure_remote_note_delivery_target(
                &mut transaction,
                status_id,
                delivery_target_account_id,
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn remote_announce_target_exists(
        &self,
        object_uri: &str,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let exists = announce_interaction_target(&mut transaction, object_uri, origin)
            .await?
            .is_some();
        transaction.commit().await?;
        Ok(exists)
    }

    pub(crate) async fn remote_announce_is_relevant(
        &self,
        account_id: i64,
        delivery_target_account_id: Option<i64>,
    ) -> Result<bool, WriteError> {
        if delivery_target_account_id.is_some() {
            return Ok(true);
        }
        let mut transaction = self.pool.begin().await?;
        let relevant = remote_announce_is_relevant(&mut transaction, account_id).await?;
        transaction.commit().await?;
        Ok(relevant)
    }

    pub(crate) async fn remote_announce_is_tombstoned(
        &self,
        account_id: i64,
        activity_uri: &str,
    ) -> Result<bool, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let tombstoned =
            remote_interaction_tombstoned(&mut transaction, account_id, activity_uri).await?;
        transaction.commit().await?;
        Ok(tombstoned)
    }

    pub(crate) async fn upsert_remote_actor(
        &self,
        username: &str,
        domain: &str,
        limited_federation: bool,
        actor: &RemoteActor,
    ) -> Result<i64, WriteError> {
        if username.trim().is_empty()
            || domain.trim().is_empty()
            || !actor.username.eq_ignore_ascii_case(username)
        {
            return Err(WriteError::InvalidInput("remote actor handle is invalid"));
        }
        self.with_remote_domain_locks(domain, || async {
            self.upsert_remote_actor_locked(username, domain, limited_federation, actor, None)
                .await
        })
        .await
    }

    /// Refresh only an existing, still-identical remote row; operator recovery must
    /// never create an account or replace its authentication keys.
    pub(crate) async fn refresh_remote_actor(
        &self,
        account_id: i64,
        username: &str,
        domain: &str,
        limited_federation: bool,
        actor: &RemoteActor,
    ) -> Result<i64, WriteError> {
        if username.trim().is_empty() || !actor.username.eq_ignore_ascii_case(username) {
            return Err(WriteError::Validation("remote refresh handle changed"));
        }
        self.with_remote_domain_locks(domain, || async {
            self.upsert_remote_actor_locked(
                username,
                domain,
                limited_federation,
                actor,
                Some(account_id),
            )
            .await
        })
        .await
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn upsert_remote_actor_locked(
        &self,
        username: &str,
        domain: &str,
        limited_federation: bool,
        actor: &RemoteActor,
        existing_id: Option<i64>,
    ) -> Result<i64, WriteError> {
        let mut transaction = self.pool.begin().await?;
        // Split-domain actors: both the account domain and the actor host must pass.
        let actor_host = crate::remote::canonical_remote_domain_from_url(&actor.id)
            .map_err(|_| WriteError::InvalidInput("remote actor URI is invalid"))?;
        for checked in [domain, actor_host.as_str()] {
            if !remote_domain_allowed_in_transaction(&mut transaction, checked, limited_federation)
                .await?
            {
                return Err(WriteError::Validation("remote actor domain is not allowed"));
            }
        }
        let uri = actor.id.as_str();
        let profile_url = actor.profile_url.as_ref().map_or(uri, Url::as_str);

        // Mastodon does not make accounts.uri unique, so serialize remote writes by actor URI.
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(
                pg_catalog.hashtextextended($1, 0)
             )",
        )
        .bind(format!("rustodon:actor:{uri}"))
        .execute(&mut *transaction)
        .await?;

        let uri_account_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM accounts
             WHERE uri = $1
             ORDER BY id
             LIMIT 1
             FOR UPDATE",
        )
        .bind(uri)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(expected_id) = existing_id {
            let same_remote: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = $1 AND domain = $2 AND uri = $3 AND lower(username) = lower($4))"
            ).bind(expected_id).bind(domain).bind(uri).bind(username).fetch_one(&mut *transaction).await?;
            if uri_account_id != Some(expected_id) || !same_remote {
                return Err(WriteError::Validation("remote refresh identity changed"));
            }
        }
        let handle_account = sqlx::query_as::<_, (i64, Option<String>)>(
            "SELECT id, uri FROM accounts
             WHERE lower(username) = lower($1) AND lower(domain) = lower($2)
             ORDER BY id
             LIMIT 1
             FOR UPDATE",
        )
        .bind(username)
        .bind(domain)
        .fetch_optional(&mut *transaction)
        .await?;
        let inserted_account_id = if uri_account_id.is_none() && handle_account.is_none() {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO accounts (
                        username, domain, actor_type, display_name, note, uri, url,
                        inbox_url, shared_inbox_url, protocol, public_key, last_webfingered_at,
                        created_at, updated_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1, '',
                               clock_timestamp(), clock_timestamp(), clock_timestamp())
                     ON CONFLICT DO NOTHING
                  RETURNING id, created_at",
            )
            .bind(username)
            .bind(domain)
            .bind(&actor.actor_type)
            .bind(&actor.display_name)
            .bind(&actor.note)
            .bind(uri)
            .bind(profile_url)
            .bind(actor.inbox.as_str())
            .bind(actor.shared_inbox.as_ref().map_or("", Url::as_str))
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            None
        };
        let concurrent_handle_account = if inserted_account_id.is_none()
            && uri_account_id.is_none()
            && handle_account.is_none()
        {
            sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT id, uri FROM accounts
                 WHERE lower(username) = lower($1) AND lower(domain) = lower($2)
                 ORDER BY id
                 LIMIT 1
                 FOR UPDATE",
            )
            .bind(username)
            .bind(domain)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            None
        };
        let account_id = remote_actor_account_id(
            uri_account_id,
            handle_account.as_ref(),
            inserted_account_id,
            concurrent_handle_account,
            uri,
        )?;
        sqlx::query(
            "UPDATE accounts SET username = $2, domain = $3, actor_type = $4,
                display_name = $5, note = $6, uri = $7, url = $8, inbox_url = $9,
                 shared_inbox_url = $10, protocol = 1,
                 public_key = CASE WHEN $13 THEN public_key ELSE '' END,
                followers_url = COALESCE($11, followers_url),
                following_url = COALESCE($12, following_url),
                last_webfingered_at = clock_timestamp(),
                updated_at = clock_timestamp()
             WHERE id = $1",
        )
        .bind(account_id)
        .bind(username)
        .bind(domain)
        .bind(&actor.actor_type)
        .bind(&actor.display_name)
        .bind(&actor.note)
        .bind(uri)
        .bind(profile_url)
        .bind(actor.inbox.as_str())
        .bind(actor.shared_inbox.as_ref().map_or("", Url::as_str))
        .bind(actor.followers.as_ref().map(Url::as_str))
        .bind(actor.following.as_ref().map(Url::as_str))
        .bind(existing_id.is_some())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE accounts SET
                suspended_at = CASE
                    WHEN $2 THEN COALESCE(suspended_at, clock_timestamp())
                    WHEN suspension_origin = 1 THEN NULL
                    ELSE suspended_at
                END,
                suspension_origin = CASE
                    WHEN $2 AND suspension_origin IS DISTINCT FROM 0 THEN 1
                    WHEN NOT $2 AND suspension_origin = 1 THEN NULL
                    ELSE suspension_origin
                END,
                updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(account_id)
        .bind(actor.suspended)
        .execute(&mut *transaction)
        .await?;
        if existing_id.is_none() {
            reconcile_remote_actor_keypairs(&mut transaction, account_id, actor).await?;
        }
        crate::mastodon::profile_media::persist_images(
            &mut transaction,
            account_id,
            actor.avatar.as_ref(),
            actor.header.as_ref(),
            true,
        )
        .await?;
        ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        transaction.commit().await?;
        Ok(account_id)
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_actor_update(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
    ) -> Result<(), WriteError> {
        let object_uri = remote_actor_object_uri(object, "id")?
            .ok_or(WriteError::InvalidInput("remote actor update has no ID"))?;
        if object_uri != actor_uri {
            return Err(WriteError::InvalidInput(
                "remote actor update identity does not match its signer",
            ));
        }
        let username = remote_actor_text(object, "preferredUsername")?;
        let display_name = remote_actor_text(object, "name")?;
        let note = remote_actor_text(object, "summary")?;
        let url = remote_actor_object_uri(object, "url")?;
        let inbox_url = remote_actor_object_uri(object, "inbox")?;
        let outbox_url = remote_actor_object_uri(object, "outbox")?;
        let followers_url = remote_actor_object_uri(object, "followers")?;
        let following_url = remote_actor_object_uri(object, "following")?;
        let (shared_inbox_set, shared_inbox_url) = match object.get("endpoints") {
            None => (false, None),
            Some(Value::Object(endpoints)) => (
                true,
                remote_actor_object_uri(&Value::Object(endpoints.clone()), "sharedInbox")?,
            ),
            Some(_) => {
                return Err(WriteError::InvalidInput(
                    "remote actor endpoints are invalid",
                ));
            }
        };
        let actor_type = object
            .get("type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let locked = object
            .get("manuallyApprovesFollowers")
            .map(|value| {
                value.as_bool().ok_or(WriteError::InvalidInput(
                    "remote actor approval policy is invalid",
                ))
            })
            .transpose()?;
        let discoverable = remote_actor_bool(object, "discoverable")?;
        let indexable = remote_actor_bool(object, "indexable")?;
        let suspended = remote_actor_bool(object, "suspended")?.unwrap_or(false);
        let fields = remote_actor_fields(object)?;
        let also_known_as = remote_actor_aliases(object)?;
        let avatar_set = object.get("icon").is_some();
        let avatar_remote_url =
            crate::mastodon::activitypub_inbox::actor_image_uri(object.get("icon"))
                .map_err(|_| WriteError::InvalidInput("remote actor image URI is invalid"))?;
        let header_set = object.get("image").is_some();
        let header_remote_url =
            crate::mastodon::activitypub_inbox::actor_image_uri(object.get("image"))
                .map_err(|_| WriteError::InvalidInput("remote actor image URI is invalid"))?;

        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        let account =
            sqlx::query_as::<_, (Option<String>, String, Option<NaiveDateTime>, Option<i32>)>(
                "SELECT domain, uri, suspended_at, suspension_origin
               FROM accounts WHERE id = $1 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?;
        let Some((domain, current_uri, suspended_at, suspension_origin)) = account else {
            return Ok(());
        };
        if domain.is_none()
            || current_uri != actor_uri
            || suspension_origin == Some(0)
            || (suspended_at.is_some() && suspension_origin != Some(1))
        {
            return Ok(());
        }
        let route_status_ids = if suspended == suspended_at.is_some() {
            Vec::new()
        } else {
            account_timeline_status_ids(&mut transaction, account_id).await?
        };
        let route_before = status_timeline_snapshots(&mut transaction, &route_status_ids).await?;
        let route_version = if route_status_ids.is_empty() {
            None
        } else {
            Some(
                sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                    .fetch_one(&mut *transaction)
                    .await?
                    .and_utc()
                    .timestamp_micros(),
            )
        };
        if suspended && let Some(version) = route_version {
            for status_id in &route_status_ids {
                collect_status_lifecycle_recipient_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    *status_id,
                    "delete",
                    StreamEventLogicalKey::Version(version),
                )
                .await?;
            }
        }
        upsert_remote_emojis(
            &mut transaction,
            domain.as_deref().expect("remote account has a domain"),
            actor_uri,
            object,
        )
        .await?;
        if let Some(note) = note.as_deref() {
            update_account_tags(&mut transaction, account_id, note).await?;
        }
        sqlx::query(
            "UPDATE accounts SET
                username = COALESCE($2, username),
                actor_type = COALESCE($3, actor_type),
                display_name = COALESCE($4, display_name),
                note = COALESCE($5, note),
                url = COALESCE($6, url),
                inbox_url = COALESCE($7, inbox_url),
                outbox_url = COALESCE($8, outbox_url),
                followers_url = COALESCE($9, followers_url),
                following_url = COALESCE($10, following_url),
                shared_inbox_url = CASE WHEN $11 THEN COALESCE($12, '') ELSE shared_inbox_url END,
                locked = COALESCE($13, locked),
                discoverable = COALESCE($14, discoverable),
                indexable = COALESCE($15, indexable),
                fields = COALESCE($16, fields),
                also_known_as = COALESCE($17, also_known_as),
                updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(account_id)
        .bind(username)
        .bind(actor_type)
        .bind(display_name)
        .bind(note)
        .bind(url)
        .bind(inbox_url)
        .bind(outbox_url)
        .bind(followers_url)
        .bind(following_url)
        .bind(shared_inbox_set)
        .bind(shared_inbox_url)
        .bind(locked)
        .bind(discoverable)
        .bind(indexable)
        .bind(fields)
        .bind(also_known_as)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE accounts SET
                suspended_at = CASE
                    WHEN $2 THEN COALESCE(suspended_at, clock_timestamp())
                    WHEN suspension_origin = 1 THEN NULL
                    ELSE suspended_at
                END,
                suspension_origin = CASE
                    WHEN $2 AND suspension_origin IS DISTINCT FROM 0 THEN 1
                    WHEN NOT $2 AND suspension_origin = 1 THEN NULL
                    ELSE suspension_origin
                END,
                updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(account_id)
        .bind(suspended)
        .execute(&mut *transaction)
        .await?;
        crate::mastodon::profile_media::persist_images(
            &mut transaction,
            account_id,
            avatar_set.then_some(&avatar_remote_url),
            header_set.then_some(&header_remote_url),
            false,
        )
        .await?;
        if let Some(route_version) = route_version {
            let route_after =
                status_timeline_snapshots(&mut transaction, &route_status_ids).await?;
            if !suspended {
                for status_id in &route_status_ids {
                    collect_status_lifecycle_recipient_stream_events(
                        &mut transaction,
                        &mut pending_stream_events,
                        *status_id,
                        "update",
                        StreamEventLogicalKey::Version(route_version),
                    )
                    .await?;
                }
            }
            collect_timeline_snapshot_transitions(
                &mut transaction,
                &mut pending_stream_events,
                "status.update",
                &format!("remote-actor:{account_id}"),
                route_version,
                &route_before,
                &route_after,
            )
            .await?;
        }
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_actor_delete(
        &self,
        account_id: i64,
        actor_uri: &str,
        origin: &str,
        severance_event_id: Option<i64>,
        expected_suspended_at: Option<NaiveDateTime>,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        let account = sqlx::query_as::<_, (Option<String>, String, Option<NaiveDateTime>)>(
            "SELECT domain, uri, suspended_at FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_uri, suspended_at)) = account else {
            return Ok(());
        };
        if domain.is_none()
            || current_uri != actor_uri
            || expected_suspended_at.is_some_and(|expected| suspended_at != Some(expected))
        {
            return Ok(());
        }
        let route_status_ids = account_timeline_status_ids(&mut transaction, account_id).await?;
        let mut route_snapshots =
            status_timeline_snapshots(&mut transaction, &route_status_ids).await?;

        let follows = sqlx::query_as::<
            _,
            (
                i64,
                i64,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<bool>,
                Option<bool>,
                Option<Vec<String>>,
            ),
        >(
            "SELECT follow.id, follow.account_id, follow.target_account_id, follow.uri,
                    source.domain, target.domain, follow.show_reblogs, follow.notify,
                    follow.languages
               FROM follows follow
               JOIN accounts source ON source.id = follow.account_id
               JOIN accounts target ON target.id = follow.target_account_id
              WHERE follow.account_id = $1 OR follow.target_account_id = $1
              ORDER BY follow.account_id, follow.target_account_id FOR UPDATE OF follow",
        )
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        for status_id in &route_status_ids {
            collect_status_lifecycle_recipient_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                *status_id,
                "delete",
                StreamEventLogicalKey::Version(0),
            )
            .await?;
        }
        let requests = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, target_account_id FROM follow_requests
              WHERE account_id = $1 OR target_account_id = $1
              ORDER BY account_id, target_account_id FOR UPDATE",
        )
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        for (
            follow_id,
            follow_account_id,
            follow_target_account_id,
            follow_uri,
            source_domain,
            target_domain,
            show_reblogs,
            notify,
            languages,
        ) in &follows
        {
            if let Some(severance_event_id) = severance_event_id
                && ((*follow_account_id == account_id && target_domain.is_none())
                    || (*follow_target_account_id == account_id && source_domain.is_none()))
            {
                let (local_account_id, direction) = if *follow_account_id == account_id {
                    (*follow_target_account_id, 0_i32)
                } else {
                    (*follow_account_id, 1_i32)
                };
                sqlx::query(
                    "INSERT INTO severed_relationships
                         (relationship_severance_event_id, local_account_id,
                          remote_account_id, direction, show_reblogs, notify, languages,
                          created_at, updated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, clock_timestamp(), clock_timestamp())
                     ON CONFLICT (relationship_severance_event_id, local_account_id, direction, remote_account_id)
                     DO NOTHING",
                )
                .bind(severance_event_id)
                .bind(local_account_id)
                .bind(account_id)
                .bind(direction)
                .bind(show_reblogs)
                .bind(notify)
                .bind(languages)
                .execute(&mut *transaction)
                .await?;
            }
            let Some(follow_uri) = follow_uri.as_deref().filter(|uri| !uri.is_empty()) else {
                continue;
            };
            if *follow_account_id == account_id
                && target_domain.is_none()
                && let Some(remote_delivery) = remote_relationship_delivery(
                    &mut transaction,
                    *follow_target_account_id,
                    account_id,
                    origin,
                )
                .await?
            {
                record_remote_reject_delivery(
                    &mut transaction,
                    *follow_target_account_id,
                    &remote_delivery,
                    *follow_id,
                    follow_uri,
                )
                .await?;
            } else if *follow_target_account_id == account_id
                && source_domain.is_none()
                && let Some(remote_delivery) = remote_relationship_delivery(
                    &mut transaction,
                    *follow_account_id,
                    account_id,
                    origin,
                )
                .await?
            {
                cancel_activitypub_delivery(&mut transaction, follow_uri).await?;
                record_remote_undo_follow_delivery(
                    &mut transaction,
                    *follow_account_id,
                    &remote_delivery,
                    follow_uri,
                    origin,
                )
                .await?;
            }
        }
        sqlx::query("DELETE FROM follows WHERE account_id = $1 OR target_account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 OR target_account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        let mut relationship_deltas = HashMap::new();
        for (_, source_account_id, target_account_id, ..) in &follows {
            add_account_stats_delta(
                &mut relationship_deltas,
                *source_account_id,
                AccountStatsDelta {
                    following: -1,
                    ..AccountStatsDelta::default()
                },
            );
            add_account_stats_delta(
                &mut relationship_deltas,
                *target_account_id,
                AccountStatsDelta {
                    followers: -1,
                    ..AccountStatsDelta::default()
                },
            );
        }
        apply_account_stats_deltas(&mut transaction, relationship_deltas).await?;
        for (follow_id, _, target_account_id, ..) in &follows {
            delete_activity_notifications(
                &mut transaction,
                *target_account_id,
                *follow_id,
                "Follow",
            )
            .await?;
        }
        for (request_id, target_account_id) in requests {
            delete_activity_notifications(
                &mut transaction,
                target_account_id,
                request_id,
                "FollowRequest",
            )
            .await?;
        }
        let owned_statuses = sqlx::query_as::<_, (i64, i64, Option<i64>, Option<i64>, i32)>(
            "SELECT id, account_id, reblog_of_id, in_reply_to_id, visibility
               FROM statuses
              WHERE account_id = $1 AND deleted_at IS NULL
              ORDER BY id FOR UPDATE",
        )
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        let original_status_ids = owned_statuses
            .iter()
            .filter(|(_, _, reblog_of_id, _, _)| reblog_of_id.is_none())
            .map(|(status_id, _, _, _, _)| *status_id)
            .collect::<Vec<_>>();
        let dependent_reblogs = if original_status_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as::<_, (i64, i64, Option<i64>, Option<i64>, i32)>(
                "SELECT id, account_id, reblog_of_id, in_reply_to_id, visibility
                   FROM statuses
                  WHERE reblog_of_id = ANY($1) AND deleted_at IS NULL
                  ORDER BY id FOR UPDATE",
            )
            .bind(&original_status_ids)
            .fetch_all(&mut *transaction)
            .await?
        };
        let mut affected_statuses = owned_statuses;
        for status in dependent_reblogs {
            if !affected_statuses
                .iter()
                .any(|(status_id, _, _, _, _)| *status_id == status.0)
            {
                affected_statuses.push(status);
            }
        }
        let affected_status_ids = affected_statuses
            .iter()
            .map(|(status_id, _, _, _, _)| *status_id)
            .collect::<Vec<_>>();
        delete_remote_status_notifications(&mut transaction, &affected_status_ids).await?;
        remove_favourites_for_account_and_statuses(
            &mut transaction,
            account_id,
            &affected_status_ids,
        )
        .await?;
        remove_poll_data_for_account_and_statuses(
            &mut transaction,
            account_id,
            &affected_status_ids,
            &[],
        )
        .await?;
        for status_id in &affected_status_ids {
            if let Some(snapshot) = route_snapshots.remove(status_id) {
                collect_status_delete_stream_events_with_snapshot(
                    &mut transaction,
                    &mut pending_stream_events,
                    *status_id,
                    snapshot,
                )
                .await?;
            }
        }
        if !affected_status_ids.is_empty() {
            sqlx::query(
                "UPDATE statuses SET deleted_at = COALESCE(deleted_at, clock_timestamp()),
                    updated_at = clock_timestamp() WHERE id = ANY($1)",
            )
            .bind(&affected_status_ids)
            .execute(&mut *transaction)
            .await?;
        }
        let mut status_deltas = HashMap::new();
        for (_, status_account_id, reblog_of_id, in_reply_to_id, visibility) in &affected_statuses {
            if let Some(reblog_of_id) = reblog_of_id {
                decrement_reblog_count(&mut transaction, *reblog_of_id).await?;
                if *status_account_id != account_id && *visibility != 3 {
                    add_account_stats_delta(
                        &mut status_deltas,
                        *status_account_id,
                        AccountStatsDelta {
                            statuses: -1,
                            ..AccountStatsDelta::default()
                        },
                    );
                }
            } else if *status_account_id == account_id
                && *visibility < 2
                && let Some(in_reply_to_id) = in_reply_to_id
            {
                decrement_reply_count(&mut transaction, *in_reply_to_id).await?;
            }
        }
        apply_account_stats_deltas(&mut transaction, status_deltas).await?;
        if !affected_status_ids.is_empty() {
            remove_statuses_from_account_conversations(&mut transaction, &affected_status_ids)
                .await?;
            sqlx::query(
                "DELETE FROM media_attachments
                  WHERE account_id = $1 OR status_id = ANY($2)",
            )
            .bind(account_id)
            .bind(&affected_status_ids)
            .execute(&mut *transaction)
            .await?;
            sqlx::query("DELETE FROM mentions WHERE account_id = $1 OR status_id = ANY($2)")
                .bind(account_id)
                .bind(&affected_status_ids)
                .execute(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM status_pins WHERE account_id = $1 OR status_id = ANY($2)")
                .bind(account_id)
                .bind(&affected_status_ids)
                .execute(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM bookmarks WHERE account_id = $1 OR status_id = ANY($2)")
                .bind(account_id)
                .bind(&affected_status_ids)
                .execute(&mut *transaction)
                .await?;
        }
        for query in [
            "DELETE FROM blocks WHERE account_id = $1 OR target_account_id = $1",
            "DELETE FROM mutes WHERE account_id = $1 OR target_account_id = $1",
            "DELETE FROM notification_permissions WHERE account_id = $1 OR from_account_id = $1",
            "DELETE FROM notifications WHERE from_account_id = $1",
            "DELETE FROM notification_requests WHERE from_account_id = $1",
            "DELETE FROM account_deletion_requests WHERE account_id = $1",
        ] {
            sqlx::query(query)
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query(
            "UPDATE accounts SET silenced_at = NULL,
                 suspended_at = COALESCE(suspended_at, clock_timestamp()),
                 suspension_origin = NULL, locked = false, memorial = false,
                 discoverable = false, trendable = false, display_name = '', note = '',
                 fields = '[]'::jsonb, also_known_as = ARRAY[]::text[], url = NULL,
                 inbox_url = '', outbox_url = '', followers_url = '', following_url = '',
                 shared_inbox_url = '', avatar_content_type = NULL, avatar_file_name = NULL,
                 avatar_file_size = NULL, avatar_remote_url = NULL,
                 avatar_storage_schema_version = NULL, avatar_updated_at = NULL,
                 header_content_type = NULL, header_file_name = NULL, header_file_size = NULL,
                 header_remote_url = '', header_storage_schema_version = NULL,
                 header_updated_at = NULL,
                 updated_at = clock_timestamp()
               WHERE id = $1",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        sqlx::query(
            "UPDATE account_stats SET statuses_count = 0, following_count = 0,
                followers_count = 0, updated_at = clock_timestamp()
              WHERE account_id = $1",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn apply_remote_note_create(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        self.apply_remote_note_create_with_quote_guard(
            account_id,
            actor_uri,
            object,
            delivery_target_account_id,
            origin,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn apply_remote_quote_request_instrument(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
        request_uri: &str,
        quoted_status_uri: &str,
        instrument_uri: &str,
        expected_target_status_id: i64,
        expected_target_account_id: i64,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        let guard = RemoteQuoteImportGuard {
            request_uri,
            quoted_status_uri,
            instrument_uri,
            expected_target_status_id,
            expected_target_account_id,
        };
        self.apply_remote_note_create_with_quote_guard(
            account_id,
            actor_uri,
            object,
            delivery_target_account_id,
            origin,
            Some(&guard),
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) async fn apply_remote_note_create_with_quote_guard(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
        quote_guard: Option<&RemoteQuoteImportGuard<'_>>,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        let mut note = RemoteNoteData::parse(object, actor_uri)?;
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_note(&mut transaction, &note.uri).await?;
        if let Some(guard) = quote_guard {
            if !same_remote_note_host(actor_uri, guard.request_uri)?
                || !same_remote_note_host(actor_uri, guard.instrument_uri)?
            {
                return Err(WriteError::InvalidInput(
                    "remote QuoteRequest identifiers do not match its actor host",
                ));
            }
            lock_remote_interaction(&mut transaction, guard.request_uri).await?;
            if remote_quote_request_decision_in(
                &mut transaction,
                guard.request_uri,
                actor_uri,
                guard.quoted_status_uri,
                guard.instrument_uri,
            )
            .await?
            .is_some()
            {
                transaction.commit().await?;
                return Ok(None);
            }
            let target =
                resolve_quote_target(&mut transaction, guard.quoted_status_uri, origin).await?;
            if target
                .as_ref()
                .is_none_or(|(status_id, account_id, local, _)| {
                    *status_id != guard.expected_target_status_id
                        || *account_id != guard.expected_target_account_id
                        || !*local
                        || delivery_target_account_id.is_some_and(|id| id != *account_id)
                })
            {
                transaction.commit().await?;
                return Ok(None);
            }
            let existing_status_id = remote_note_status_id_for_account(
                &mut transaction,
                account_id,
                &note.uri,
                note.atom_uri.as_deref(),
            )
            .await?;
            let mut guarded_status_ids = existing_status_id.into_iter().collect::<Vec<_>>();
            guarded_status_ids.push(guard.expected_target_status_id);
            lock_statuses_in_order(&mut transaction, &guarded_status_ids).await?;
            match writable_quote_target(
                &mut transaction,
                account_id,
                guard.expected_target_status_id,
            )
            .await
            {
                Ok(_) => {}
                Err(WriteError::NotFound | WriteError::Forbidden) => {
                    transaction.commit().await?;
                    return Ok(None);
                }
                Err(error) => return Err(error),
            }
        }
        let account = sqlx::query_as::<
            _,
            (
                Option<String>,
                String,
                String,
                String,
                Option<NaiveDateTime>,
            ),
        >(
            "SELECT domain, uri, followers_url, following_url, suspended_at FROM accounts
             WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_actor_uri, followers_url, following_url, suspended_at)) = account
        else {
            return Ok(None);
        };
        if domain.is_none() || current_actor_uri != actor_uri || suspended_at.is_some() {
            return Ok(None);
        }
        note.quote_approval_policy = remote_quote_approval_policy_with_collections(
            object
                .as_object()
                .ok_or(WriteError::InvalidInput("remote Note is not an object"))?,
            actor_uri,
            &followers_url,
            &following_url,
        )?;
        if !same_remote_note_host(actor_uri, &note.uri)? {
            return Err(WriteError::InvalidInput(
                "remote Note URI does not match its actor host",
            ));
        }
        let tombstoned = remote_note_tombstoned(
            &mut transaction,
            account_id,
            &note.uri,
            note.atom_uri.as_deref(),
        )
        .await?;
        if tombstoned {
            transaction.commit().await?;
            return Ok(None);
        }
        let existing_status_id = remote_note_status_id_for_account(
            &mut transaction,
            account_id,
            &note.uri,
            note.atom_uri.as_deref(),
        )
        .await?;
        prelock_remote_note_quote_targets(&mut transaction, existing_status_id, &note, origin)
            .await?;
        let existing =
            remote_note_status(&mut transaction, &note.uri, note.atom_uri.as_deref()).await?;
        if let Some((existing_status_id, existing_account_id, _, _, _)) = existing {
            if existing_account_id != account_id {
                return Err(WriteError::Conflict);
            }
            if let Some(delivery_target_account_id) = delivery_target_account_id {
                ensure_remote_note_delivery_target(
                    &mut transaction,
                    existing_status_id,
                    delivery_target_account_id,
                )
                .await?;
            }
            transaction.commit().await?;
            return Ok(None);
        }
        upsert_remote_emojis(
            &mut transaction,
            domain.as_deref().expect("remote accounts have a domain"),
            actor_uri,
            object,
        )
        .await?;
        let visibility = remote_note_visibility(&note.audience, &followers_url);
        let (in_reply_to_id, in_reply_to_account_id, conversation_id) =
            remote_note_thread(&mut transaction, &note, origin).await?;
        let status_id = insert_remote_note(
            &mut transaction,
            account_id,
            &note,
            visibility,
            in_reply_to_id,
            in_reply_to_account_id,
            conversation_id,
        )
        .await?;
        if let Some(poll) = note.poll.as_ref() {
            upsert_remote_poll(
                &mut transaction,
                status_id,
                account_id,
                poll,
                true,
                false,
                false,
            )
            .await?;
        }
        if visibility < 2
            && let Some(in_reply_to_id) = in_reply_to_id
        {
            increment_reply_count(&mut transaction, in_reply_to_id).await?;
        }
        let conversation_id = ensure_remote_note_conversation(
            &mut transaction,
            status_id,
            account_id,
            in_reply_to_id,
            in_reply_to_account_id,
            conversation_id,
            note.conversation_uri.as_deref(),
            note.published_at,
        )
        .await?;
        if conversation_id.is_some() {
            sqlx::query("UPDATE statuses SET conversation_id = $1 WHERE id = $2")
                .bind(conversation_id)
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
        }
        let media_ids =
            insert_remote_note_media(&mut transaction, status_id, account_id, &note.attachments)
                .await?;
        sqlx::query("UPDATE statuses SET ordered_media_attachment_ids = $2 WHERE id = $1")
            .bind(status_id)
            .bind(&media_ids)
            .execute(&mut *transaction)
            .await?;
        let mention_ids = insert_remote_note_mentions(
            &mut transaction,
            status_id,
            &note,
            delivery_target_account_id,
            origin,
        )
        .await?;
        reconcile_remote_note_quote(&mut transaction, status_id, account_id, &note, origin).await?;
        // Classify only after resolving mentions, including an implicit inbox recipient.
        // An explicitly mentioned Note is direct only when no silent recipient was added.
        let visibility = if visibility == 4
            && remote_note_has_only_explicit_recipients(&mut transaction, status_id, &note).await?
        {
            sqlx::query("UPDATE statuses SET visibility = 3 WHERE id = $1")
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
            3
        } else {
            visibility
        };
        update_remote_note_tags(&mut transaction, status_id, &note.hashtags).await?;
        insert_remote_note_stats(&mut transaction, status_id, &note).await?;
        if visibility == 3 {
            ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        } else {
            increment_account_status_count(&mut transaction, account_id, note.published_at).await?;
        }
        if note.in_reply_to_uri.is_some() && in_reply_to_id.is_none() {
            let thread_job = JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND,
                json!({
                    "child_status_id": status_id,
                    "parent_url": note.in_reply_to_uri.as_deref()
                }),
            )
            .logical_key(format!("activitypub:thread:{status_id}"))
            .max_attempts(4);
            record_outbox_once_in(&mut transaction, &thread_job).await?;
        }
        for (mention_id, recipient_account_id) in &mention_ids {
            record_outbox_in(
                &mut transaction,
                &notification_job(*recipient_account_id, NOTIFICATION_MENTION, *mention_id),
            )
            .await?;
        }
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "update",
            note.published_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(Some(RemoteNoteWriteOutcome {
            status_id,
            mention_ids,
        }))
    }

    pub(crate) async fn record_remote_note_forwarding(
        &self,
        actor_uri: &str,
        object: &Value,
        activity: &Value,
    ) -> Result<(), WriteError> {
        let note = RemoteNoteData::parse(object, actor_uri)?;
        self.record_remote_activity_forwarding(
            actor_uri,
            &note.uri,
            note.atom_uri.as_deref(),
            activity,
        )
        .await
    }

    pub(crate) async fn record_remote_note_reference_forwarding(
        &self,
        actor_uri: &str,
        object_uri: &str,
        activity: &Value,
    ) -> Result<(), WriteError> {
        self.record_remote_activity_forwarding(actor_uri, object_uri, None, activity)
            .await
    }

    pub(crate) async fn record_remote_note_delete_forwarding(
        &self,
        actor_uri: &str,
        object_uri: &str,
        atom_uri: Option<&str>,
        activity: &Value,
    ) -> Result<(), WriteError> {
        self.record_remote_activity_forwarding(actor_uri, object_uri, atom_uri, activity)
            .await
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn record_remote_activity_forwarding(
        &self,
        actor_uri: &str,
        object_uri: &str,
        atom_uri: Option<&str>,
        activity: &Value,
    ) -> Result<(), WriteError> {
        let activity_uri = activity
            .get("id")
            .and_then(Value::as_str)
            .filter(|uri| !uri.trim().is_empty())
            .ok_or(WriteError::InvalidInput("signed remote activity has no ID"))?;
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote activity object URI does not match its actor host",
            ));
        }
        if let Some(atom_uri) = atom_uri
            && !same_remote_note_host(actor_uri, atom_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote activity atom URI does not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let Some((status_id, parent_account_id, source_inbox)) =
            sqlx::query_as::<_, (i64, Option<i64>, String)>(
                "SELECT status.id, parent.id,
                        COALESCE(NULLIF(source.shared_inbox_url, ''), source.inbox_url)
                   FROM statuses status
                   JOIN accounts source ON source.id = status.account_id
                                        AND source.uri = $2
                                         AND source.domain IS NOT NULL
              LEFT JOIN statuses parent_status ON parent_status.id = status.in_reply_to_id
                                               AND parent_status.deleted_at IS NULL
              LEFT JOIN accounts parent ON parent.id = parent_status.account_id
                                        AND parent.domain IS NULL
                  WHERE (status.uri = $1
                         OR ($3::text IS NOT NULL AND status.uri = $3))
                    AND status.deleted_at IS NULL
                    AND status.visibility IN (0, 1)
                  LIMIT 1",
            )
            .bind(object_uri)
            .bind(actor_uri)
            .bind(atom_uri)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        record_remote_activity_forwarding_for_status_in(
            &mut transaction,
            status_id,
            parent_account_id,
            &source_inbox,
            activity_uri,
            activity,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn resolve_remote_note_thread(
        &self,
        child_status_id: i64,
        parent_uri: &str,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let parsed_parent_url = Url::parse(parent_uri)
            .map_err(|_| WriteError::InvalidInput("remote reply parent URI is invalid"))?;
        if !matches!(parsed_parent_url.scheme(), "http" | "https")
            || parsed_parent_url.host_str().is_none()
        {
            return Err(WriteError::InvalidInput(
                "remote reply parent URI is invalid",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let child = sqlx::query_as::<_, (i64, i32, Option<i64>, Option<NaiveDateTime>)>(
            "SELECT account_id, visibility, in_reply_to_id, deleted_at
               FROM statuses WHERE id = $1 FOR UPDATE",
        )
        .bind(child_status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((child_account_id, child_visibility, current_parent_id, child_deleted_at)) = child
        else {
            transaction.commit().await?;
            return Ok(true);
        };
        if child_deleted_at.is_some() || current_parent_id.is_some() {
            transaction.commit().await?;
            return Ok(true);
        }
        let parent = sqlx::query_as::<
            _,
            (
                i64,
                i64,
                Option<i64>,
                bool,
                Option<i64>,
                Option<NaiveDateTime>,
            ),
        >(
            "SELECT status.id, status.account_id, status.conversation_id, status.reply,
                    status.in_reply_to_account_id, status.deleted_at
               FROM statuses status
               JOIN accounts author ON author.id = status.account_id
              WHERE status.deleted_at IS NULL
                AND (
                  status.uri = $1
                  OR status.url = $1
                  OR (author.domain IS NULL AND (
                    $1 = $2 || '/actor/statuses/' || status.id::text
                         || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                    OR $1 = $2 || '/@' || author.username || '/' || status.id::text
                         || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                    OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text
                         || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                    OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text
                         || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                  ))
                )
              ORDER BY status.id
              LIMIT 1 FOR UPDATE",
        )
        .bind(parent_uri)
        .bind(origin.trim_end_matches('/'))
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((
            parent_id,
            parent_account_id,
            parent_conversation_id,
            parent_reply,
            parent_reply_account_id,
            parent_deleted_at,
        )) = parent
        else {
            transaction.commit().await?;
            return Ok(false);
        };
        if parent_deleted_at.is_some() {
            transaction.commit().await?;
            return Ok(true);
        }
        if parent_id == child_status_id {
            return Err(WriteError::InvalidInput(
                "remote reply parent cannot be the child status",
            ));
        }
        let carried_reply_account_id = if parent_reply && parent_account_id == child_account_id {
            parent_reply_account_id.or(Some(parent_account_id))
        } else {
            Some(parent_account_id)
        };
        sqlx::query(
            "UPDATE statuses SET reply = true, in_reply_to_id = $2,
                in_reply_to_account_id = $3,
                conversation_id = COALESCE(conversation_id, $4),
                updated_at = clock_timestamp()
              WHERE id = $1 AND in_reply_to_id IS NULL",
        )
        .bind(child_status_id)
        .bind(parent_id)
        .bind(carried_reply_account_id)
        .bind(parent_conversation_id)
        .execute(&mut *transaction)
        .await?;
        if child_visibility < 2 {
            increment_reply_count(&mut transaction, parent_id).await?;
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub(crate) async fn apply_remote_note_update(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        self.apply_remote_note_update_with_authority(
            account_id,
            actor_uri,
            object,
            delivery_target_account_id,
            origin,
            RemoteUpdateAuthority::Inbox,
            None,
        )
        .await
    }

    pub(crate) async fn apply_signed_remote_poll_refresh(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        origin: &str,
        expected_poll_id: i64,
        expected_poll_lock_version: i32,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        self.apply_remote_note_update_with_authority(
            account_id,
            actor_uri,
            object,
            None,
            origin,
            RemoteUpdateAuthority::SignedRefresh,
            Some((expected_poll_id, expected_poll_lock_version)),
        )
        .await
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn apply_signed_remote_poll_refresh_for_test(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        origin: &str,
        expected_poll_id: i64,
        expected_poll_lock_version: i32,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        self.apply_signed_remote_poll_refresh(
            account_id,
            actor_uri,
            object,
            origin,
            expected_poll_id,
            expected_poll_lock_version,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    pub(super) async fn apply_remote_note_update_with_authority(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
        authority: RemoteUpdateAuthority,
        expected_poll: Option<(i64, i32)>,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        let mut note = RemoteNoteData::parse(object, actor_uri)?;
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_note(&mut transaction, &note.uri).await?;
        if !same_remote_note_host(actor_uri, &note.uri)? {
            return Err(WriteError::InvalidInput(
                "remote Note URI does not match its actor host",
            ));
        }
        let account = sqlx::query_as::<
            _,
            (
                Option<String>,
                String,
                String,
                String,
                Option<NaiveDateTime>,
            ),
        >(
            "SELECT domain, uri, followers_url, following_url, suspended_at FROM accounts
             WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_actor_uri, followers_url, following_url, suspended_at)) = account
        else {
            return Ok(None);
        };
        if domain.is_none() || current_actor_uri != actor_uri || suspended_at.is_some() {
            return Ok(None);
        }
        note.quote_approval_policy = remote_quote_approval_policy_with_collections(
            object
                .as_object()
                .ok_or(WriteError::InvalidInput("remote Note is not an object"))?,
            actor_uri,
            &followers_url,
            &following_url,
        )?;
        let existing_status_id = remote_note_status_id_for_account(
            &mut transaction,
            account_id,
            &note.uri,
            note.atom_uri.as_deref(),
        )
        .await?;
        prelock_remote_note_quote_targets(&mut transaction, existing_status_id, &note, origin)
            .await?;
        let Some((
            status_id,
            existing_account_id,
            deleted_at,
            current_edited_at,
            current_created_at,
        )) = remote_note_status(&mut transaction, &note.uri, note.atom_uri.as_deref()).await?
        else {
            let tombstoned = remote_note_tombstoned(
                &mut transaction,
                account_id,
                &note.uri,
                note.atom_uri.as_deref(),
            )
            .await?;
            transaction.commit().await?;
            if tombstoned {
                return Ok(None);
            }
            if remote_note_object_is_too_old(object, note.published_at, Utc::now().naive_utc()) {
                return Ok(None);
            }
            return self
                .apply_remote_note_create(
                    account_id,
                    actor_uri,
                    object,
                    delivery_target_account_id,
                    origin,
                )
                .await;
        };
        if existing_account_id != account_id {
            return Err(WriteError::Conflict);
        }
        if deleted_at.is_some() {
            transaction.commit().await?;
            return Ok(None);
        }
        if let Some((expected_poll_id, expected_lock_version)) = expected_poll {
            let matching_poll_id = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM polls \
                 WHERE id = $1 AND status_id = $2 AND lock_version = $3 FOR UPDATE",
            )
            .bind(expected_poll_id)
            .bind(status_id)
            .bind(expected_lock_version)
            .fetch_optional(&mut *transaction)
            .await?;
            if matching_poll_id != Some(expected_poll_id) {
                transaction.commit().await?;
                return Err(WriteError::Conflict);
            }
        }
        if note.edited_at.is_none() {
            // Unsolicited inbox objects cannot roll an explicitly versioned status back. A
            // directly signed poll refresh is authoritative only for unchanged poll shape/tallies.
            if current_edited_at.is_some() && authority == RemoteUpdateAuthority::Inbox {
                transaction.commit().await?;
                return Ok(None);
            }
            let quote_changed =
                reconcile_remote_note_quote(&mut transaction, status_id, account_id, &note, origin)
                    .await?;
            let poll_reconcile =
                if authority == RemoteUpdateAuthority::SignedRefresh && note.poll.is_none() {
                    RemotePollReconcile::Unchanged
                } else {
                    reconcile_remote_poll(
                        &mut transaction,
                        status_id,
                        account_id,
                        note.poll.as_ref(),
                        false,
                        authority.rejects_tally_regression(),
                        authority.claims_freshness(),
                    )
                    .await?
                };
            update_remote_note_stats(&mut transaction, status_id, &note).await?;
            if quote_changed {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    note.updated_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            if let RemotePollReconcile::Tally(updated_at) = poll_reconcile {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    updated_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
            transaction.commit().await?;
            return Ok(None);
        }
        let current_version = current_edited_at.unwrap_or(current_created_at);
        if note.updated_at < current_version {
            transaction.commit().await?;
            return Ok(None);
        }
        if note.updated_at == current_version {
            let quote_changed =
                reconcile_remote_note_quote(&mut transaction, status_id, account_id, &note, origin)
                    .await?;
            let poll_reconcile =
                if authority == RemoteUpdateAuthority::SignedRefresh && note.poll.is_none() {
                    RemotePollReconcile::Unchanged
                } else {
                    reconcile_remote_poll(
                        &mut transaction,
                        status_id,
                        account_id,
                        note.poll.as_ref(),
                        false,
                        authority.rejects_tally_regression(),
                        authority.claims_freshness(),
                    )
                    .await?
                };
            update_remote_note_stats(&mut transaction, status_id, &note).await?;
            if quote_changed {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    note.updated_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            if let RemotePollReconcile::Tally(updated_at) = poll_reconcile {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    updated_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
            transaction.commit().await?;
            return Ok(None);
        }
        let poll_reconcile = reconcile_remote_poll(
            &mut transaction,
            status_id,
            account_id,
            note.poll.as_ref(),
            true,
            authority.rejects_tally_regression(),
            authority.claims_freshness(),
        )
        .await?;
        let html_origin =
            Url::parse(origin).map_err(|_| WriteError::InvalidInput("local origin is invalid"))?;
        let formatter =
            HtmlFormatter::new(&html_origin, html_origin.host_str().unwrap_or_default());
        let route_before = status_timeline_snapshot(&mut transaction, status_id).await?;
        let before = remote_note_edit_projection(&mut transaction, status_id, &formatter).await?;
        upsert_remote_emojis(
            &mut transaction,
            domain.as_deref().expect("remote accounts have a domain"),
            actor_uri,
            object,
        )
        .await?;
        sqlx::query(
            "UPDATE statuses SET text = $2, spoiler_text = $3, sensitive = $4,
                language = $5, quote_approval_policy = $6, updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(status_id)
        .bind(&note.content)
        .bind(&note.summary)
        .bind(note.sensitive)
        .bind(&note.language)
        .bind(note.quote_approval_policy)
        .execute(&mut *transaction)
        .await?;
        remove_remote_note_media_not_in(&mut transaction, status_id, &note.attachments).await?;
        let old_mentions = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, account_id FROM mentions WHERE status_id = $1 ORDER BY account_id, id",
        )
        .bind(status_id)
        .fetch_all(&mut *transaction)
        .await?;
        for (mention_id, recipient_account_id) in &old_mentions {
            cancel_pending_notification_jobs(&mut transaction, *recipient_account_id, *mention_id)
                .await?;
        }
        sqlx::query(
            "UPDATE mentions SET silent = true, updated_at = clock_timestamp() \
             WHERE status_id = $1",
        )
        .bind(status_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM statuses_tags WHERE status_id = $1")
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
        let media_ids =
            insert_remote_note_media(&mut transaction, status_id, account_id, &note.attachments)
                .await?;
        sqlx::query("UPDATE statuses SET ordered_media_attachment_ids = $2 WHERE id = $1")
            .bind(status_id)
            .bind(&media_ids)
            .execute(&mut *transaction)
            .await?;
        let mention_ids = insert_remote_note_mentions(
            &mut transaction,
            status_id,
            &note,
            delivery_target_account_id,
            origin,
        )
        .await?;
        let quote_changed =
            reconcile_remote_note_quote(&mut transaction, status_id, account_id, &note, origin)
                .await?;
        update_remote_note_tags(&mut transaction, status_id, &note.hashtags).await?;
        update_remote_note_stats(&mut transaction, status_id, &note).await?;
        for (mention_id, recipient_account_id) in &mention_ids {
            record_outbox_in(
                &mut transaction,
                &notification_job(*recipient_account_id, NOTIFICATION_MENTION, *mention_id),
            )
            .await?;
        }
        let projection_changed =
            remote_note_edit_projection(&mut transaction, status_id, &formatter).await? != before;
        let route_after = status_timeline_snapshot(&mut transaction, status_id).await?;
        // Route-only edits (for example hashtag or language changes) still need a timeline
        // transition even when the rendered status projection is byte-for-byte unchanged.
        let meaningful_update = quote_changed
            || projection_changed
            || route_after != route_before
            || poll_reconcile == RemotePollReconcile::Significant;
        if !meaningful_update {
            if let RemotePollReconcile::Tally(updated_at) = poll_reconcile {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    updated_at.and_utc().timestamp_micros(),
                )
                .await?;
                flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
            }
            transaction.commit().await?;
            return Ok(None);
        }
        sqlx::query("UPDATE statuses SET edited_at = $2 WHERE id = $1")
            .bind(status_id)
            .bind(note.updated_at)
            .execute(&mut *transaction)
            .await?;
        record_status_update_notifications(
            &mut transaction,
            status_id,
            note.updated_at.and_utc().timestamp_micros(),
        )
        .await?;
        collect_status_stream_transition(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            StreamEventLogicalKey::Version(note.updated_at.and_utc().timestamp_micros()),
            Some(route_before),
            Some(route_after),
        )
        .await?;
        collect_status_update_notification_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            note.updated_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(Some(RemoteNoteWriteOutcome {
            status_id,
            mention_ids,
        }))
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_note_delete(
        &self,
        account_id: i64,
        actor_uri: &str,
        object_uri: &str,
        atom_uri: Option<&str>,
        origin: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_quote_status_deletion(&mut transaction).await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_note(&mut transaction, object_uri).await?;
        let account = sqlx::query_as::<_, (Option<String>, String, Option<NaiveDateTime>)>(
            "SELECT domain, uri, suspended_at FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_actor_uri, _)) = account else {
            return Ok(());
        };
        if domain.is_none() || current_actor_uri != actor_uri {
            return Ok(());
        }
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Delete URI does not match its actor host",
            ));
        }
        if let Some(atom_uri) = atom_uri
            && !same_remote_note_host(actor_uri, atom_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote Delete atom URI does not match its actor host",
            ));
        }
        let status_id =
            remote_note_status_id_for_account(&mut transaction, account_id, object_uri, atom_uri)
                .await?;
        if let Some(status_id) = status_id {
            let mut quote_lifecycle_status_ids = sqlx::query_scalar::<_, i64>(
                "SELECT quote.status_id FROM quotes quote
                   JOIN statuses quoting ON quoting.id = quote.status_id
                  WHERE quote.quoted_status_id = $1 AND quoting.deleted_at IS NULL
                  ORDER BY quote.status_id",
            )
            .bind(status_id)
            .fetch_all(&mut *transaction)
            .await?;
            quote_lifecycle_status_ids.push(status_id);
            lock_statuses_in_order(&mut transaction, &quote_lifecycle_status_ids).await?;
            let status = sqlx::query_as::<_, (i64, Option<NaiveDateTime>, i32, Option<i64>)>(
                "SELECT account_id, deleted_at, visibility, in_reply_to_id
                   FROM statuses WHERE id = $1 FOR UPDATE",
            )
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some((existing_account_id, deleted_at, visibility, in_reply_to_id)) = status {
                if existing_account_id != account_id {
                    return Err(WriteError::Conflict);
                }
                let owned_quote = sqlx::query_as::<
                    _,
                    (i64, Option<i64>, Option<i64>, i32, Option<String>, bool),
                >(
                    "SELECT quote.id, quote.quoted_status_id, quote.quoted_account_id,
                                quote.state, quote.activity_uri,
                                COALESCE(target.domain IS NULL, false)
                           FROM quotes quote
                      LEFT JOIN accounts target ON target.id = quote.quoted_account_id
                          WHERE quote.status_id = $1 FOR UPDATE OF quote",
                )
                .bind(status_id)
                .fetch_optional(&mut *transaction)
                .await?;
                let quoting_quotes = sqlx::query_as::<_, (i64, i64, Option<String>)>(
                    "SELECT quote.id, quote.status_id, quote.activity_uri FROM quotes quote \
                       JOIN statuses quoting ON quoting.id = quote.status_id \
                      WHERE quote.quoted_status_id = $1 AND quoting.deleted_at IS NULL \
                      ORDER BY quote.id FOR UPDATE OF quote",
                )
                .bind(status_id)
                .fetch_all(&mut *transaction)
                .await?;
                if !quoting_quotes.is_empty() {
                    // Before detaching: the notification cleanup below can no longer
                    // find quoting statuses once quoted_status_id is NULL.
                    let quoting_status_ids = quoting_quotes
                        .iter()
                        .map(|(_, quoting_status_id, _)| *quoting_status_id)
                        .collect::<Vec<_>>();
                    delete_quoted_update_notifications(&mut transaction, &quoting_status_ids)
                        .await?;
                    sqlx::query(
                        "UPDATE quotes SET quoted_status_id = NULL, approval_uri = NULL, \
                                updated_at = clock_timestamp() \
                         WHERE quoted_status_id = $1",
                    )
                    .bind(status_id)
                    .execute(&mut *transaction)
                    .await?;
                    for (quote_id, quoting_status_id, request_uri) in quoting_quotes {
                        cancel_quote_request_outbox(
                            &mut transaction,
                            quote_id,
                            request_uri.as_deref(),
                        )
                        .await?;
                        let version = if sqlx::query_scalar::<_, bool>(
                            "SELECT account.domain IS NULL FROM statuses quoting \
                             JOIN accounts account ON account.id = quoting.account_id \
                             WHERE quoting.id = $1",
                        )
                        .bind(quoting_status_id)
                        .fetch_one(&mut *transaction)
                        .await?
                        {
                            record_quote_status_update(&mut transaction, quoting_status_id)
                                .await?
                                .and_utc()
                                .timestamp_micros()
                        } else {
                            Utc::now().timestamp_micros()
                        };
                        collect_status_stream_events(
                            &mut transaction,
                            &mut pending_stream_events,
                            quoting_status_id,
                            "status.update",
                            version,
                        )
                        .await?;
                    }
                }
                let reblogs = sqlx::query_as::<_, (i64, i64, i32)>(
                    "SELECT id, account_id, visibility FROM statuses
                      WHERE reblog_of_id = $1 AND deleted_at IS NULL
                      ORDER BY id FOR UPDATE",
                )
                .bind(status_id)
                .fetch_all(&mut *transaction)
                .await?;
                let reblog_ids = reblogs.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
                let local_reblog_ids = sqlx::query_scalar::<_, i64>(
                    "SELECT status.id FROM statuses status
                       JOIN accounts account ON account.id = status.account_id
                      WHERE status.id = ANY($1) AND status.local IS TRUE
                        AND account.domain IS NULL
                      ORDER BY status.id",
                )
                .bind(&reblog_ids)
                .fetch_all(&mut *transaction)
                .await?;
                let mut status_deltas = HashMap::new();
                add_account_stats_delta(
                    &mut status_deltas,
                    account_id,
                    AccountStatsDelta::default(),
                );
                if !reblogs.is_empty() {
                    sqlx::query(
                        "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                          WHERE id = ANY($1)",
                    )
                    .bind(&reblog_ids)
                    .execute(&mut *transaction)
                    .await?;
                    for (_, reblog_account_id, reblog_visibility) in &reblogs {
                        if *reblog_visibility != 3 {
                            add_account_stats_delta(
                                &mut status_deltas,
                                *reblog_account_id,
                                AccountStatsDelta {
                                    statuses: -1,
                                    ..AccountStatsDelta::default()
                                },
                            );
                        }
                        decrement_reblog_count(&mut transaction, status_id).await?;
                    }
                    for (reblog_id, _, _) in &reblogs {
                        collect_status_delete_stream_events(
                            &mut transaction,
                            &mut pending_stream_events,
                            *reblog_id,
                        )
                        .await?;
                    }
                }
                for reblog_id in local_reblog_ids {
                    record_status_delete_distribution(&mut transaction, reblog_id, &[]).await?;
                }
                if deleted_at.is_none() {
                    if let Some((
                        quote_id,
                        quoted_status_id,
                        quoted_account_id,
                        state,
                        request_uri,
                        quoted_account_local,
                    )) = owned_quote
                    {
                        if state == 1
                            && let Some(quoted_status_id) = quoted_status_id
                        {
                            decrement_quote_count(&mut transaction, quoted_status_id).await?;
                            if quoted_account_local
                                && let Some(quoted_account_id) = quoted_account_id
                            {
                                record_quote_authorization_delete(
                                    &mut transaction,
                                    quote_id,
                                    status_id,
                                    quoted_status_id,
                                    quoted_account_id,
                                    origin,
                                )
                                .await?;
                            }
                        }
                        if let Some(quoted_account_id) = quoted_account_id {
                            delete_activity_notifications(
                                &mut transaction,
                                quoted_account_id,
                                quote_id,
                                "Quote",
                            )
                            .await?;
                        }
                        cancel_quote_request_outbox(
                            &mut transaction,
                            quote_id,
                            request_uri.as_deref(),
                        )
                        .await?;
                    }
                    sqlx::query(
                        "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                         WHERE id = $1",
                    )
                    .bind(status_id)
                    .execute(&mut *transaction)
                    .await?;
                    if visibility != 3 {
                        add_account_stats_delta(
                            &mut status_deltas,
                            account_id,
                            AccountStatsDelta {
                                statuses: -1,
                                ..AccountStatsDelta::default()
                            },
                        );
                    }
                    if visibility < 2
                        && let Some(in_reply_to_id) = in_reply_to_id
                    {
                        decrement_reply_count(&mut transaction, in_reply_to_id).await?;
                    }
                }
                apply_account_stats_deltas(&mut transaction, status_deltas).await?;
                collect_status_delete_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                )
                .await?;
            }
            let mut affected_status_ids = vec![status_id];
            affected_status_ids.extend(
                sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM statuses WHERE reblog_of_id = $1 ORDER BY id",
                )
                .bind(status_id)
                .fetch_all(&mut *transaction)
                .await?,
            );
            delete_remote_status_notifications(&mut transaction, &affected_status_ids).await?;
            remove_favourites_for_statuses(&mut transaction, &affected_status_ids).await?;
            remove_poll_data_for_statuses(&mut transaction, &affected_status_ids).await?;
            remove_statuses_from_account_conversations(&mut transaction, &affected_status_ids)
                .await?;
            sqlx::query("DELETE FROM media_attachments WHERE status_id = ANY($1::bigint[])")
                .bind(&affected_status_ids)
                .execute(&mut *transaction)
                .await?;
        }
        insert_remote_note_tombstone(&mut transaction, account_id, object_uri).await?;
        if let Some(atom_uri) = atom_uri.filter(|value| *value != object_uri) {
            insert_remote_note_tombstone(&mut transaction, account_id, atom_uri).await?;
        }
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn remote_undo_reference_kind(
        &self,
        account_id: i64,
        activity_uri: &str,
    ) -> Result<RemoteUndoReferenceKind, WriteError> {
        let kind = sqlx::query_scalar::<_, i32>(
            "SELECT CASE
                WHEN EXISTS (
                    SELECT 1 FROM statuses
                     WHERE account_id = $1 AND uri = $2
                       AND reblog_of_id IS NOT NULL AND deleted_at IS NULL
                ) THEN 2
                WHEN EXISTS (
                    SELECT 1 FROM follows WHERE account_id = $1 AND uri = $2
                ) OR EXISTS (
                    SELECT 1 FROM follow_requests WHERE account_id = $1 AND uri = $2
                ) THEN 0
                WHEN EXISTS (
                    SELECT 1 FROM blocks WHERE account_id = $1 AND uri = $2
                ) THEN 1
                ELSE 3
             END",
        )
        .bind(account_id)
        .bind(activity_uri)
        .fetch_one(&self.pool)
        .await?;
        Ok(match kind {
            0 => RemoteUndoReferenceKind::Follow,
            1 => RemoteUndoReferenceKind::Block,
            2 => RemoteUndoReferenceKind::Announce,
            _ => RemoteUndoReferenceKind::Unknown,
        })
    }

    pub(crate) async fn apply_remote_like(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
        object_uri: &str,
        origin: &str,
    ) -> Result<Option<RemoteInteractionWriteOutcome>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Like URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, true).await? {
            return Ok(None);
        }
        if remote_interaction_tombstoned(&mut transaction, account_id, activity_uri).await? {
            transaction.commit().await?;
            return Ok(None);
        }
        let Some((status_id, recipient_account_id, _)) =
            local_interaction_target(&mut transaction, object_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        let favourite_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO favourites (account_id, status_id, created_at, updated_at)
             VALUES ($1, $2, clock_timestamp(), clock_timestamp())
             ON CONFLICT (account_id, status_id) DO NOTHING RETURNING id",
        )
        .bind(account_id)
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(favourite_id) = favourite_id else {
            let favourite_id = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM favourites WHERE account_id = $1 AND status_id = $2",
            )
            .bind(account_id)
            .bind(status_id)
            .fetch_one(&mut *transaction)
            .await?;
            return Ok(Some(RemoteInteractionWriteOutcome {
                activity_id: favourite_id,
                recipient_account_id,
            }));
        };
        increment_favourite_count(&mut transaction, status_id).await?;
        record_outbox_in(
            &mut transaction,
            &notification_job(recipient_account_id, NOTIFICATION_FAVOURITE, favourite_id),
        )
        .await?;
        transaction.commit().await?;
        Ok(Some(RemoteInteractionWriteOutcome {
            activity_id: favourite_id,
            recipient_account_id,
        }))
    }

    pub(crate) async fn apply_remote_undo_like(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
        object_uri: &str,
        origin: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Undo Like URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, false).await?
        {
            return Ok(());
        }
        let Some((status_id, _, _)) =
            local_interaction_target(&mut transaction, object_uri, origin).await?
        else {
            insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
            transaction.commit().await?;
            return Ok(());
        };
        let Some((favourite_id, recipient_account_id)) = sqlx::query_as::<_, (i64, i64)>(
            "DELETE FROM favourites favourite USING statuses status
              WHERE favourite.account_id = $1 AND favourite.status_id = status.id
                AND status.id = $2 RETURNING favourite.id, status.account_id",
        )
        .bind(account_id)
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
            transaction.commit().await?;
            return Ok(());
        };
        decrement_favourite_count(&mut transaction, status_id).await?;
        delete_activity_notifications(
            &mut transaction,
            recipient_account_id,
            favourite_id,
            "Favourite",
        )
        .await?;
        insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn apply_remote_announce(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
        object_uri: &str,
        to: &[String],
        cc: &[String],
        published_at: Option<&str>,
        origin: &str,
    ) -> Result<Option<RemoteInteractionWriteOutcome>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Announce URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, true).await? {
            return Ok(None);
        }
        if remote_interaction_tombstoned(&mut transaction, account_id, activity_uri).await? {
            transaction.commit().await?;
            return Ok(None);
        }
        let Some(outer_status_target) =
            announce_interaction_target(&mut transaction, object_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        let (
            target_status_id,
            recipient_account_id,
            target_visibility,
            target_account_is_local,
            original_account_is_local,
        ) = outer_status_target;
        if !target_account_is_local
            && !remote_announce_is_relevant(&mut transaction, account_id).await?
        {
            transaction.commit().await?;
            return Ok(None);
        }
        if let Some((existing_boost_id, existing_account_id, existing_target_id)) =
            sqlx::query_as::<_, (i64, i64, Option<i64>)>(
                "SELECT id, account_id, reblog_of_id FROM statuses WHERE uri = $1 FOR UPDATE",
            )
            .bind(activity_uri)
            .fetch_optional(&mut *transaction)
            .await?
        {
            if existing_account_id != account_id || existing_target_id != Some(target_status_id) {
                return Err(WriteError::Conflict);
            }
            transaction.commit().await?;
            return Ok(Some(RemoteInteractionWriteOutcome {
                activity_id: existing_boost_id,
                recipient_account_id,
            }));
        }
        if let Some(existing_boost_id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses
             WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL
             ORDER BY id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(account_id)
        .bind(target_status_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            transaction.commit().await?;
            return Ok(Some(RemoteInteractionWriteOutcome {
                activity_id: existing_boost_id,
                recipient_account_id,
            }));
        }
        if target_visibility > 1 && recipient_account_id != account_id {
            return Err(WriteError::InvalidInput(
                "remote Announce target is not distributable",
            ));
        }
        let followers_url = sqlx::query_scalar::<_, String>(
            "SELECT followers_url FROM accounts WHERE id = $1 AND uri = $2 FOR UPDATE",
        )
        .bind(account_id)
        .bind(actor_uri)
        .fetch_one(&mut *transaction)
        .await?;
        let visibility = remote_interaction_visibility(to, cc, &followers_url);
        if visibility > 2 {
            return Err(WriteError::InvalidInput(
                "remote Announce visibility is not distributable",
            ));
        }
        let created_at = remote_interaction_timestamp(published_at)?;
        let boost_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO statuses (
                account_id, text, spoiler_text, visibility, local, sensitive, reply,
                reblog_of_id, uri, url, created_at, updated_at
             ) VALUES ($1, '', '', $4, false, false, false, $2, $3, NULL,
                         $5, clock_timestamp()) RETURNING id",
        )
        .bind(account_id)
        .bind(target_status_id)
        .bind(activity_uri)
        .bind(visibility)
        .bind(created_at)
        .fetch_one(&mut *transaction)
        .await?;
        let conversation_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO conversations (created_at, parent_account_id, parent_status_id, updated_at, uri)
             VALUES (clock_timestamp(), $1, $2, clock_timestamp(), NULL)
             RETURNING id",
        )
        .bind(account_id)
        .bind(boost_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("UPDATE statuses SET conversation_id = $1 WHERE id = $2")
            .bind(conversation_id)
            .bind(boost_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO status_stats (status_id, created_at, updated_at)
             VALUES ($1, clock_timestamp(), clock_timestamp())",
        )
        .bind(boost_id)
        .execute(&mut *transaction)
        .await?;
        if visibility == 3 {
            ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        } else {
            increment_account_status_count(&mut transaction, account_id, created_at).await?;
        }
        increment_reblog_count(&mut transaction, target_status_id).await?;
        if original_account_is_local
            && !remote_announce_notification_suppressed(
                &mut transaction,
                account_id,
                recipient_account_id,
            )
            .await?
        {
            record_outbox_in(
                &mut transaction,
                &notification_job(recipient_account_id, NOTIFICATION_REBLOG, boost_id),
            )
            .await?;
        }
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            boost_id,
            "update",
            created_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(Some(RemoteInteractionWriteOutcome {
            activity_id: boost_id,
            recipient_account_id,
        }))
    }

    pub(crate) async fn apply_remote_undo_announce(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
        object_uri: &str,
        origin: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Undo Announce URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, false).await?
        {
            return Ok(());
        }
        let Some((target_status_id, _, _, _, _)) =
            announce_interaction_target(&mut transaction, object_uri, origin).await?
        else {
            insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
            transaction.commit().await?;
            return Ok(());
        };
        if let Some((boost_id, target_status_id, visibility, recipient_account_id)) =
            sqlx::query_as::<_, (i64, i64, i32, i64)>(
                "SELECT boost.id, boost.reblog_of_id, boost.visibility, target.account_id
              FROM statuses boost
              JOIN statuses target ON target.id = boost.reblog_of_id
              WHERE boost.account_id = $1 AND boost.uri = $2
                AND target.id = $3 AND boost.deleted_at IS NULL
              LIMIT 1 FOR UPDATE",
            )
            .bind(account_id)
            .bind(activity_uri)
            .bind(target_status_id)
            .fetch_optional(&mut *transaction)
            .await?
        {
            sqlx::query(
                "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                 WHERE id = $1",
            )
            .bind(boost_id)
            .execute(&mut *transaction)
            .await?;
            if visibility == 3 {
                ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
            } else {
                decrement_account_status_count(&mut transaction, account_id).await?;
            }
            decrement_reblog_count(&mut transaction, target_status_id).await?;
            delete_activity_notifications(
                &mut transaction,
                recipient_account_id,
                boost_id,
                "Status",
            )
            .await?;
            collect_status_delete_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                boost_id,
            )
            .await?;
        }
        insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn apply_remote_undo_announce_reference(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Undo reference URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, false).await?
        {
            return Ok(());
        }
        if let Some((boost_id, target_status_id, visibility, recipient_account_id)) =
            sqlx::query_as::<_, (i64, i64, i32, i64)>(
                "SELECT boost.id, boost.reblog_of_id, boost.visibility, target.account_id
                 FROM statuses boost
                 JOIN statuses target ON target.id = boost.reblog_of_id
                 WHERE boost.account_id = $1 AND boost.uri = $2
                   AND boost.deleted_at IS NULL AND target.deleted_at IS NULL
                 LIMIT 1",
            )
            .bind(account_id)
            .bind(activity_uri)
            .fetch_optional(&mut *transaction)
            .await?
        {
            let target_is_live = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM statuses WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
            )
            .bind(target_status_id)
            .fetch_optional(&mut *transaction)
            .await?
            .is_some();
            let boost_is_live = target_is_live
                && sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM statuses WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
                )
                .bind(boost_id)
                .fetch_optional(&mut *transaction)
                .await?
                .is_some();
            if !boost_is_live {
                insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
                transaction.commit().await?;
                return Ok(());
            }
            sqlx::query(
                "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                 WHERE id = $1",
            )
            .bind(boost_id)
            .execute(&mut *transaction)
            .await?;
            if visibility == 3 {
                ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
            } else {
                decrement_account_status_count(&mut transaction, account_id).await?;
            }
            decrement_reblog_count(&mut transaction, target_status_id).await?;
            delete_activity_notifications(
                &mut transaction,
                recipient_account_id,
                boost_id,
                "Status",
            )
            .await?;
            collect_status_delete_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                boost_id,
            )
            .await?;
        }
        insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod split_domain_tests {
    use super::*;

    fn actor(id: &str, username: &str, domain: &str) -> RemoteActor {
        let id = Url::parse(id).unwrap();
        RemoteActor {
            inbox: id.join("inbox").unwrap(),
            id,
            username: username.to_owned(),
            domain: domain.to_owned(),
            actor_type: "Person".to_owned(),
            display_name: String::new(),
            note: String::new(),
            suspended: false,
            profile_url: None,
            avatar: None,
            header: None,
            shared_inbox: None,
            followers: None,
            following: None,
            public_keys: Vec::new(),
            key_set_complete: true,
        }
    }

    #[tokio::test]
    #[ignore = "requires the disposable worker PostgreSQL fixture"]
    async fn split_domain_actors_store_the_account_domain_and_honor_host_blocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let owner = PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
        let writer =
            WriteRepository::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
        sqlx::query(
            "INSERT INTO domain_blocks (domain, severity, reject_media, reject_reports, \
               obfuscate, created_at, updated_at) \
             VALUES ('evil.split.invalid', 1, false, false, false, now(), now()) \
             ON CONFLICT (domain) DO NOTHING",
        )
        .execute(&owner)
        .await?;
        let blocked = actor(
            "https://evil.split.invalid/users/m",
            "m",
            "clean.split.invalid",
        );
        assert!(
            writer
                .upsert_remote_actor("m", "clean.split.invalid", false, &blocked)
                .await
                .is_err(),
            "a blocked actor host must not hide behind an allowed account domain"
        );
        let split = actor(
            "https://social.split.invalid/users/jae",
            "jae",
            "split.invalid",
        );
        let id = writer
            .upsert_remote_actor("jae", "split.invalid", false, &split)
            .await?;
        let domain: Option<String> =
            sqlx::query_scalar("SELECT domain FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_one(&owner)
                .await?;
        assert_eq!(domain.as_deref(), Some("split.invalid"));
        Ok(())
    }
}
