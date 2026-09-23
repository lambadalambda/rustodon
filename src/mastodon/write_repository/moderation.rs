//! Moderation writes: reports, suspensions, account deletion and domain blocks.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

#[allow(clippy::missing_errors_doc)]
impl WriteRepository {
    /// Creates one user report and queues the first staff notification atomically.
    ///
    /// # Errors
    ///
    /// Returns a write error when the authenticated account, target, or attached records are not
    /// valid, or when `PostgreSQL` rejects the transaction.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn create_report(
        &self,
        authenticated: &AuthenticatedBearer,
        target_account_id: i64,
        comment: &str,
        category: Option<&str>,
        status_ids: &[i64],
        collection_ids: &[i64],
        rule_ids: &[i64],
        forward: Option<bool>,
        forward_to_domains: Option<&[String]>,
        origin: &str,
        report_mail_enabled: bool,
    ) -> Result<i64, WriteError> {
        let comment = if comment.trim().is_empty() {
            ""
        } else {
            comment
        };
        if comment.chars().count() > 1_000 {
            return Err(WriteError::Validation("comment is too long"));
        }
        let category = report_category_value(category, !rule_ids.is_empty())?;
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_REPORTS)
            .await?;
        let Some((target_remote, target_unavailable, source_local, target_domain)) =
            sqlx::query_as::<_, (bool, bool, bool, Option<String>)>(
                "SELECT target.domain IS NOT NULL, \
                        target.suspended_at IS NOT NULL AND target.id <> -99, \
                        source.domain IS NULL, target.domain \
                 FROM accounts source JOIN accounts target ON target.id = $2 \
                 WHERE source.id = $1 FOR UPDATE OF target",
            )
            .bind(account_id)
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(WriteError::NotFound);
        };
        if target_unavailable {
            return Err(WriteError::NotFound);
        }
        let forward_to_domains = forward_to_domains.map_or_else(
            || target_domain.iter().cloned().collect(),
            ToOwned::to_owned,
        );

        let invalid_status = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM unnest($1::bigint[]) requested(id) \
                WHERE NOT EXISTS ( \
                    SELECT 1 FROM statuses status \
                    WHERE status.id = requested.id AND status.account_id = $2 \
                      AND NOT EXISTS ( \
                    SELECT 1 FROM blocks blocked \
                        WHERE blocked.account_id = $2 AND blocked.target_account_id = $3) \
                      AND ( \
                        status.reblog_of_id IS NULL OR NOT EXISTS ( \
                            SELECT 1 \
                              FROM statuses original \
                              JOIN accounts original_account ON original_account.id = original.account_id \
                             WHERE original.id = status.reblog_of_id \
                               AND ( \
                                 EXISTS ( \
                                     SELECT 1 FROM blocks blocked_original \
                                      WHERE blocked_original.account_id = $3 \
                                        AND blocked_original.target_account_id = original.account_id) \
                                 OR EXISTS ( \
                                     SELECT 1 FROM blocks blocking_original \
                                      WHERE blocking_original.account_id = original.account_id \
                                        AND blocking_original.target_account_id = $3) \
                                 OR EXISTS ( \
                                     SELECT 1 FROM mutes muted_original \
                                      WHERE muted_original.account_id = $3 \
                                        AND muted_original.target_account_id = original.account_id) \
                                 OR EXISTS ( \
                                     SELECT 1 FROM account_domain_blocks blocked_domain \
                                      WHERE blocked_domain.account_id = $3 \
                                        AND original_account.domain IS NOT NULL \
                                        AND lower(blocked_domain.domain) = lower(original_account.domain))))) \
                      AND ( \
                        $3 = $2 OR status.visibility IN (0, 1) \
                        OR (status.visibility = 2 AND EXISTS ( \
                            SELECT 1 FROM follows follow \
                            WHERE follow.account_id = $3 AND follow.target_account_id = $2)) \
                        OR (status.visibility IN (2, 3, 4) AND EXISTS ( \
                            SELECT 1 FROM mentions mention \
                            WHERE mention.status_id = status.id AND mention.account_id = $3)))))",
        )
        .bind(status_ids)
        .bind(target_account_id)
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if invalid_status {
            return Err(WriteError::NotFound);
        }

        let invalid_collection = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM unnest($1::bigint[]) requested(id) \
                WHERE NOT EXISTS ( \
                    SELECT 1 FROM collections collection \
                    WHERE collection.id = requested.id AND collection.account_id = $2))",
        )
        .bind(collection_ids)
        .bind(target_account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if invalid_collection {
            return Err(WriteError::NotFound);
        }
        if !rule_ids.is_empty() {
            if rule_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != rule_ids.len()
            {
                return Err(WriteError::Validation("invalid rules"));
            }
            let invalid_rule = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS ( \
                    SELECT 1 FROM unnest($1::bigint[]) requested(id) \
                    WHERE NOT EXISTS (SELECT 1 FROM rules rule WHERE rule.id = requested.id))",
            )
            .bind(rule_ids)
            .fetch_one(&mut *transaction)
            .await?;
            if invalid_rule {
                return Err(WriteError::Validation("invalid rules"));
            }
        }

        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock( \
                 pg_catalog.hashtextextended($1, 0))",
        )
        .bind(format!("rustodon.report_rate_limit:{account_id}"))
        .execute(&mut *transaction)
        .await?;
        let report_count: i64 = sqlx::query_scalar(REPORT_RATE_LIMIT_COUNT_SQL)
            .bind(account_id)
            .fetch_one(&mut *transaction)
            .await?;
        if report_count >= REPORT_RATE_LIMIT {
            return Err(WriteError::RateLimited);
        }

        let unresolved_sibling = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM reports report \
                WHERE report.target_account_id = $1 AND report.action_taken_at IS NULL)",
        )
        .bind(target_account_id)
        .fetch_one(&mut *transaction)
        .await?;
        let report_uri = source_local.then(|| {
            format!(
                "{}/payloads/{}",
                origin.trim_end_matches('/'),
                random_uuid()
            )
        });
        let forwarded = forward.map(|requested| {
            requested
                && target_remote
                && target_domain.as_deref().is_some_and(|domain| {
                    forward_to_domains
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(domain))
                })
        });
        let report_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO reports ( \
                account_id, target_account_id, application_id, category, comment, forwarded, \
                rule_ids, status_ids, uri, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, \
                     clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(account_id)
        .bind(target_account_id)
        .bind(authenticated.application_id())
        .bind(category)
        .bind(comment)
        .bind(forwarded)
        .bind((!rule_ids.is_empty()).then_some(rule_ids.to_vec()))
        .bind(status_ids)
        .bind(report_uri)
        .fetch_one(&mut *transaction)
        .await?;

        for collection_id in collection_ids {
            sqlx::query(
                "INSERT INTO collection_reports \
                    (collection_id, report_id, created_at, updated_at) \
                 VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
            )
            .bind(collection_id)
            .bind(report_id)
            .execute(&mut *transaction)
            .await?;
        }

        if !unresolved_sibling {
            let (reporter_username, reporter_domain, target_username, target_domain) =
                sqlx::query_as::<_, (String, Option<String>, String, Option<String>)>(
                    "SELECT reporter.username, reporter.domain, target.username, target.domain \
                     FROM accounts reporter JOIN accounts target ON target.id = $2 \
                     WHERE reporter.id = $1",
                )
                .bind(account_id)
                .bind(target_account_id)
                .fetch_one(&mut *transaction)
                .await?;
            let target_label = report_account_label(&target_username, target_domain.as_deref());
            let reporter_label =
                report_account_label(&reporter_username, reporter_domain.as_deref());
            let staff_accounts = report_staff_accounts(&mut transaction).await?;
            for (staff_account_id, email, settings) in staff_accounts {
                record_outbox_in(
                    &mut transaction,
                    &notification_job(staff_account_id, "admin.report", report_id),
                )
                .await?;
                if report_mail_enabled && report_email_enabled(settings.as_deref()) {
                    let mail = report_job(
                        &email,
                        origin,
                        report_id,
                        &target_label,
                        &reporter_label,
                        staff_account_id,
                    );
                    record_outbox_in(&mut transaction, &mail).await?;
                }
            }
        }
        if target_remote && forward.unwrap_or(false) {
            record_report_forwarding(&mut transaction, report_id, origin, &forward_to_domains)
                .await?;
        }
        transaction.commit().await?;
        Ok(report_id)
    }

    /// Creates a report received from a remote `ActivityPub` actor.
    ///
    /// Returns `Ok(None)` when the source domain rejects reports or none of the
    /// Flag objects identify a known target account.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn create_remote_report(
        &self,
        source_account_id: i64,
        object_uris: &[String],
        comment: &str,
        report_uri: Option<&str>,
        origin: &str,
        local_domain: &str,
        report_mail_enabled: bool,
    ) -> Result<Option<i64>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_account_scope(&mut transaction, source_account_id).await?;
        let Some((source_domain, source_suspended)) = sqlx::query_as::<_, (String, bool)>(
            "SELECT domain, suspended_at IS NOT NULL
               FROM accounts
              WHERE id = $1 AND domain IS NOT NULL",
        )
        .bind(source_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Err(WriteError::NotFound);
        };
        if source_suspended {
            return Ok(None);
        }
        for scope in remote_domain_lock_scopes(&source_domain) {
            lock_domain_scope(&mut transaction, &scope).await?;
        }
        let source_suspended = sqlx::query_scalar::<_, bool>(
            "SELECT suspended_at IS NOT NULL FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(source_account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if source_suspended {
            return Ok(None);
        }
        let policy_domain = domain_policy_hostname(&source_domain);
        let rejects_reports = sqlx::query_scalar::<_, bool>(
            "SELECT COALESCE((
                 SELECT reject_reports
                   FROM domain_blocks
                  WHERE lower(domain) = lower(trim(trailing '.' FROM $1))
                     OR lower(trim(trailing '.' FROM $1)) LIKE '%.' || lower(domain)
                  ORDER BY char_length(domain) DESC
                  LIMIT 1
               ), false)",
        )
        .bind(&policy_domain)
        .fetch_one(&mut *transaction)
        .await?;
        if rejects_reports {
            return Ok(None);
        }

        let mut target_account_ids = Vec::new();
        for object_uri in object_uris {
            if let Some(account_id) =
                local_activitypub_account_id(&mut transaction, object_uri, origin).await?
                && !target_account_ids.contains(&account_id)
            {
                target_account_ids.push(account_id);
            }
        }
        if target_account_ids.is_empty() {
            return Ok(None);
        }
        target_account_ids.sort_unstable();
        let comment = comment.chars().take(5_000).collect::<String>();
        let report_uri = report_uri.filter(|uri| report_uri_matches_domain(uri, &source_domain));
        let mut last_report_id = None;
        for target_account_id in target_account_ids {
            lock_account_scope(&mut transaction, target_account_id).await?;
            let unavailable = sqlx::query_scalar::<_, bool>(
                "SELECT suspended_at IS NOT NULL FROM accounts WHERE id = $1 FOR UPDATE",
            )
            .bind(target_account_id)
            .fetch_one(&mut *transaction)
            .await?;
            if unavailable {
                continue;
            }
            let (status_ids, collection_ids) = remote_report_target_content(
                &mut transaction,
                target_account_id,
                &source_domain,
                origin,
                local_domain,
                object_uris,
            )
            .await?;
            let unresolved_sibling = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                     SELECT 1 FROM reports report
                      WHERE report.target_account_id = $1 AND report.action_taken_at IS NULL
                 )",
            )
            .bind(target_account_id)
            .fetch_one(&mut *transaction)
            .await?;
            let report_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO reports (
                     account_id, target_account_id, application_id, category, comment, forwarded,
                     rule_ids, status_ids, uri, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                         clock_timestamp(), clock_timestamp())
                 RETURNING id",
            )
            .bind(source_account_id)
            .bind(target_account_id)
            .bind(Option::<i64>::None)
            .bind(0_i32)
            .bind(&comment)
            .bind(false)
            .bind(Option::<Vec<i64>>::None)
            .bind(&status_ids)
            .bind(report_uri)
            .fetch_one(&mut *transaction)
            .await?;

            for collection_id in collection_ids {
                sqlx::query(
                    "INSERT INTO collection_reports
                        (collection_id, report_id, created_at, updated_at)
                     VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
                )
                .bind(collection_id)
                .bind(report_id)
                .execute(&mut *transaction)
                .await?;
            }

            if !unresolved_sibling {
                let (reporter_username, reporter_domain, target_username, target_domain) =
                    sqlx::query_as::<_, (String, Option<String>, String, Option<String>)>(
                        "SELECT reporter.username, reporter.domain, target.username, target.domain \
                         FROM accounts reporter JOIN accounts target ON target.id = $2 \
                         WHERE reporter.id = $1",
                    )
                    .bind(source_account_id)
                    .bind(target_account_id)
                    .fetch_one(&mut *transaction)
                    .await?;
                let target_label = report_account_label(&target_username, target_domain.as_deref());
                let reporter_label =
                    report_account_label(&reporter_username, reporter_domain.as_deref());
                for (staff_account_id, email, settings) in
                    report_staff_accounts(&mut transaction).await?
                {
                    record_outbox_in(
                        &mut transaction,
                        &notification_job(staff_account_id, "admin.report", report_id),
                    )
                    .await?;
                    if report_mail_enabled && report_email_enabled(settings.as_deref()) {
                        let mail = report_job(
                            &email,
                            origin,
                            report_id,
                            &target_label,
                            &reporter_label,
                            staff_account_id,
                        );
                        record_outbox_in(&mut transaction, &mail).await?;
                    }
                }
            }
            last_report_id = Some(report_id);
        }
        transaction.commit().await?;
        Ok(last_report_id)
    }

    /// Returns the number of reports created by an account in the current Mastodon rate window.
    ///
    /// # Errors
    ///
    /// Returns a database error when the report window cannot be inspected.
    pub async fn report_rate_limit_count(&self, account_id: i64) -> Result<i64, WriteError> {
        Ok(sqlx::query_scalar(REPORT_RATE_LIMIT_COUNT_SQL)
            .bind(account_id)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Resolves or reopens a report for an authorized moderation account.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account cannot manage reports,
    /// [`WriteError::NotFound`] when the report does not exist, or a database error when the
    /// report and audit log cannot be updated atomically.
    pub async fn set_report_resolution(
        &self,
        acting_account_id: i64,
        report_id: i64,
        resolved: bool,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let can_manage_reports = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 FROM users account_user \
                 JOIN accounts account ON account.id = account_user.account_id \
                 JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
                 LEFT JOIN user_roles everyone ON everyone.id = -99 \
                 WHERE account.id = $1 AND account.domain IS NULL \
                   AND account.suspended_at IS NULL \
                   AND account_user.confirmed_at IS NOT NULL \
                   AND account_user.approved = true \
                   AND account_user.disabled = false \
                   AND (role.permissions & 1 <> 0 OR \
                        ((role.permissions | COALESCE(everyone.permissions, 0)) & $2 <> 0)) \
             )",
        )
        .bind(acting_account_id)
        .bind(1_i64 << 4)
        .fetch_one(&mut *transaction)
        .await?;
        if !can_manage_reports {
            return Err(WriteError::Unauthorized);
        }

        let target_account_id =
            sqlx::query_scalar::<_, i64>("SELECT target_account_id FROM reports WHERE id = $1")
                .bind(report_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(WriteError::NotFound)?;
        sqlx::query("SELECT id FROM accounts WHERE id = $1 FOR UPDATE")
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(WriteError::NotFound)?;
        let report_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM reports WHERE id = $1 FOR UPDATE)",
        )
        .bind(report_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !report_exists {
            return Err(WriteError::NotFound);
        }

        if resolved {
            sqlx::query(
                "UPDATE reports SET action_taken_at = clock_timestamp(), \
                        action_taken_by_account_id = $2, updated_at = clock_timestamp() \
                 WHERE id = $1",
            )
            .bind(report_id)
            .bind(acting_account_id)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "UPDATE reports SET action_taken_at = NULL, action_taken_by_account_id = NULL, \
                        updated_at = clock_timestamp() \
                 WHERE id = $1",
            )
            .bind(report_id)
            .execute(&mut *transaction)
            .await?;
        }

        let action = if resolved { "resolve" } else { "reopen" };
        sqlx::query(
            "INSERT INTO admin_action_logs ( \
                 account_id, action, created_at, human_identifier, route_param, target_id, \
                 target_type, updated_at) \
             VALUES ($1, $2, clock_timestamp(), $3, NULL, $4, 'Report', clock_timestamp())",
        )
        .bind(acting_account_id)
        .bind(action)
        .bind(report_id.to_string())
        .bind(report_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Suspends or unsuspends an account for an authorized moderation account.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account cannot perform the requested
    /// action, [`WriteError::NotFound`] for an unknown or instance account, or a database error
    /// when the account, audit record, and local actor update cannot commit atomically.
    #[allow(clippy::too_many_lines)]
    pub async fn set_account_suspension(
        &self,
        acting_account_id: i64,
        account_id: i64,
        suspended: bool,
        origin: &str,
    ) -> Result<(), WriteError> {
        let permission_mask = if suspended {
            (1_i64 << 4) | (1_i64 << 10)
        } else {
            1_i64 << 10
        };
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_account_scope(&mut transaction, account_id).await?;
        let Some(actor_position) =
            authorized_admin_account(&mut transaction, acting_account_id, permission_mask).await?
        else {
            return Err(WriteError::Unauthorized);
        };
        let target = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                Option<NaiveDateTime>,
                Option<i32>,
                String,
                Option<i32>,
            ),
        >(
            "SELECT username, domain, suspended_at, suspension_origin, uri, id_scheme FROM accounts \
             WHERE id = $1 AND id <> -99 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if !suspended && target.1.is_some() {
            return Err(WriteError::InvalidInput(
                "remote account unsuspension requires a fresh remote account resolution",
            ));
        }
        if suspended {
            let target_position = sqlx::query_scalar::<_, i32>(
                "SELECT target_role.position FROM accounts target \
                 LEFT JOIN users target_user ON target_user.account_id = target.id \
                 JOIN user_roles target_role ON target_role.id = COALESCE(target_user.role_id, -99) \
                 WHERE target.id = $1",
            )
            .bind(account_id)
            .fetch_one(&mut *transaction)
            .await?;
            if actor_position <= target_position {
                return Err(WriteError::Unauthorized);
            }
        }
        if suspended {
            if target.2.is_some() {
                return Err(WriteError::Validation("account is already suspended"));
            }
        } else {
            if target.2.is_none() {
                return Err(WriteError::Validation("account is not suspended"));
            }
            if target.3 != Some(0) {
                return Err(WriteError::Unauthorized);
            }
        }

        let human_identifier = target.1.as_deref().map_or_else(
            || target.0.clone(),
            |domain| format!("{}@{domain}", target.0),
        );
        let user_email = if target.1.is_none() {
            sqlx::query_scalar::<_, String>(
                "SELECT email FROM users WHERE account_id = $1 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            None
        };
        let warning_id = if suspended {
            sqlx::query(
                "INSERT INTO account_deletion_requests (account_id, created_at, updated_at) \
                 SELECT $1, clock_timestamp(), clock_timestamp() \
                 WHERE NOT EXISTS (SELECT 1 FROM account_deletion_requests WHERE account_id = $1)",
            )
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
            let (deletion_request_id, deletion_created_at) =
                sqlx::query_as::<_, (i64, NaiveDateTime)>(
                    "SELECT id, created_at FROM account_deletion_requests \
                 WHERE account_id = $1 ORDER BY id LIMIT 1",
                )
                .bind(account_id)
                .fetch_one(&mut *transaction)
                .await?;
            if let Some(email) = user_email.as_deref() {
                sqlx::query(
                    "INSERT INTO canonical_email_blocks \
                         (canonical_email_hash, reference_account_id, created_at, updated_at) \
                     VALUES ($1, $2, clock_timestamp(), clock_timestamp()) \
                     ON CONFLICT (canonical_email_hash) DO NOTHING",
                )
                .bind(canonical_email_hash(email))
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            }
            let warning_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO account_warnings ( \
                     account_id, action, created_at, report_id, status_ids, target_account_id, text, updated_at) \
                 VALUES ($1, 4000, clock_timestamp(), NULL, NULL, $2, '', clock_timestamp()) \
                 RETURNING id",
            )
            .bind(acting_account_id)
            .bind(account_id)
            .fetch_one(&mut *transaction)
            .await?;
            let report_ids = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM reports \
                 WHERE target_account_id = $1 AND action_taken_at IS NULL \
                 ORDER BY id FOR UPDATE",
            )
            .bind(account_id)
            .fetch_all(&mut *transaction)
            .await?;
            for report_id in report_ids {
                sqlx::query(
                    "UPDATE reports SET action_taken_at = clock_timestamp(), \
                            action_taken_by_account_id = $2, updated_at = clock_timestamp() \
                     WHERE id = $1",
                )
                .bind(report_id)
                .bind(acting_account_id)
                .execute(&mut *transaction)
                .await?;
                insert_admin_action_log(
                    &mut transaction,
                    acting_account_id,
                    "resolve",
                    report_id,
                    "Report",
                    report_id.to_string(),
                    None,
                )
                .await?;
            }
            let purge_run_at =
                deletion_created_at.and_utc() + ChronoDuration::days(ACCOUNT_DELETION_DELAY_DAYS);
            if target.1.is_none() {
                let actor_path = if target.5 == Some(AccountIdScheme::Numeric.raw()) {
                    format!("ap/users/{account_id}")
                } else {
                    format!("users/{}", target.0)
                };
                let actor_uri = format!("{}/{actor_path}", origin.trim_end_matches('/'));
                record_outbox_in(
                    &mut transaction,
                    &account_delete_job(account_id, &actor_uri).run_at(purge_run_at),
                )
                .await?;
            }
            record_outbox_in(
                &mut transaction,
                &account_purge_job(
                    account_id,
                    deletion_request_id,
                    deletion_created_at,
                    target.1.is_some().then_some(origin),
                ),
            )
            .await?;
            Some(warning_id)
        } else {
            sqlx::query("DELETE FROM account_deletion_requests WHERE account_id = $1")
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            cancel_pending_account_job(
                &mut transaction,
                ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND,
                account_id,
            )
            .await?;
            cancel_pending_account_job(
                &mut transaction,
                MASTODON_ACCOUNT_PURGE_JOB_KIND,
                account_id,
            )
            .await?;
            cancel_activitypub_delivery(&mut transaction, &format!("{}#delete", target.4)).await?;
            if target.1.is_none() {
                sqlx::query("DELETE FROM canonical_email_blocks WHERE reference_account_id = $1")
                    .bind(account_id)
                    .execute(&mut *transaction)
                    .await?;
            }
            None
        };

        let transition_at =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?;
        if suspended {
            collect_account_timeline_transition(
                &mut transaction,
                &mut pending_stream_events,
                account_id,
                "delete",
                transition_at.and_utc().timestamp_micros(),
            )
            .await?;
        }
        let (domain, updated_at) = if suspended {
            sqlx::query_as::<_, (Option<String>, NaiveDateTime)>(
                "UPDATE accounts SET suspended_at = $2, suspension_origin = 0, \
                    updated_at = $2 WHERE id = $1 RETURNING domain, updated_at",
            )
            .bind(account_id)
            .bind(transition_at)
            .fetch_one(&mut *transaction)
            .await?
        } else {
            sqlx::query_as::<_, (Option<String>, NaiveDateTime)>(
                "UPDATE accounts SET suspended_at = NULL, suspension_origin = NULL, \
                    updated_at = $2 WHERE id = $1 RETURNING domain, updated_at",
            )
            .bind(account_id)
            .bind(transition_at)
            .fetch_one(&mut *transaction)
            .await?
        };
        if !suspended {
            collect_account_timeline_transition(
                &mut transaction,
                &mut pending_stream_events,
                account_id,
                "update",
                transition_at.and_utc().timestamp_micros(),
            )
            .await?;
        }
        if domain.is_none() {
            if suspended {
                collect_account_kill_stream_event(
                    &mut pending_stream_events,
                    account_id,
                    updated_at,
                )?;
            }
            record_outbox_in(
                &mut transaction,
                &account_update_job(account_id, updated_at),
            )
            .await?;
            if let Some(warning_id) = warning_id {
                record_outbox_in(
                    &mut transaction,
                    &notification_job(account_id, "AccountWarning", warning_id),
                )
                .await?;
            }
        } else if suspended {
            reject_remote_account_follows(&mut transaction, account_id, origin).await?;
        }
        let action = if suspended { "suspend" } else { "unsuspend" };
        insert_admin_action_log(
            &mut transaction,
            acting_account_id,
            action,
            account_id,
            "Account",
            human_identifier,
            None,
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Starts self-service deletion for a local account and queues its actor deletion fan-out.
    ///
    /// The browser layer verifies the account challenge before calling this method. The account
    /// suspension, deletion request, stale actor-update cancellation, and durable deletion job
    /// are committed together so a successful response cannot lose the federation intent.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::NotFound`] when the account has no local user, [`WriteError::Validation`]
    /// when the account is already unavailable or has a pending request, or a database error when
    /// the transaction cannot commit.
    pub async fn request_account_deletion(
        &self,
        account_id: i64,
        actor_uri: &str,
    ) -> Result<(), WriteError> {
        if actor_uri.is_empty() {
            return Err(WriteError::InvalidInput("account actor URI is required"));
        }
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_account_scope(&mut transaction, account_id).await?;
        let Some((domain, suspended_at)) =
            sqlx::query_as::<_, (Option<String>, Option<NaiveDateTime>)>(
                "SELECT domain, suspended_at FROM accounts WHERE id = $1 AND id <> -99 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(WriteError::NotFound);
        };
        if domain.is_some() {
            return Err(WriteError::InvalidInput(
                "remote accounts cannot be deleted locally",
            ));
        }
        if suspended_at.is_some() {
            return Err(WriteError::Validation("account is already unavailable"));
        }
        let user_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM users WHERE account_id = $1)",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !user_exists {
            return Err(WriteError::NotFound);
        }
        let deletion_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM account_deletion_requests WHERE account_id = $1)",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if deletion_exists {
            return Err(WriteError::Validation(
                "account deletion is already pending",
            ));
        }
        let (deletion_request_id, deletion_created_at) = sqlx::query_as::<_, (i64, NaiveDateTime)>(
            "INSERT INTO account_deletion_requests (account_id, created_at, updated_at) \
             VALUES ($1, clock_timestamp(), clock_timestamp()) RETURNING id, created_at",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        let transition_at =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?;
        collect_account_timeline_transition(
            &mut transaction,
            &mut pending_stream_events,
            account_id,
            "delete",
            transition_at.and_utc().timestamp_micros(),
        )
        .await?;
        let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "UPDATE accounts SET suspended_at = $2, suspension_origin = 0, \
             updated_at = $2 WHERE id = $1 RETURNING updated_at",
        )
        .bind(account_id)
        .bind(transition_at)
        .fetch_one(&mut *transaction)
        .await?;
        collect_account_kill_stream_event(&mut pending_stream_events, account_id, updated_at)?;
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
             WHERE kind = $1 AND dispatched_at IS NULL \
               AND payload -> 'arguments' ->> 'account_id' = $2",
        )
        .bind(ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND)
        .bind(account_id.to_string())
        .execute(&mut *transaction)
        .await?;
        record_outbox_in(&mut transaction, &account_delete_job(account_id, actor_uri)).await?;
        record_outbox_in(
            &mut transaction,
            &account_purge_job(account_id, deletion_request_id, deletion_created_at, None),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Purges the local content of a due self-service deletion while retaining its actor identity.
    ///
    /// The caller must hold the account lifecycle lock while collecting the filesystem manifest,
    /// invoking this method, and removing the manifest. A missing request after a suspended
    /// account has already been purged is reported separately from a canceled or not-yet-due
    /// request so stale leased jobs cannot remove media after unsuspension.
    ///
    /// # Errors
    ///
    /// Returns a database error when cleanup cannot commit atomically.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn purge_account_after_deletion(
        &self,
        account_id: i64,
        expected_deletion_request_id: Option<i64>,
        expected_deletion_created_at: Option<NaiveDateTime>,
        origin: Option<&str>,
    ) -> Result<AccountPurgeOutcome, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        let Some((domain, suspended_at)) =
            sqlx::query_as::<_, (Option<String>, Option<NaiveDateTime>)>(
                "SELECT domain, suspended_at FROM accounts WHERE id = $1 AND id <> -99 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Ok(AccountPurgeOutcome::AlreadyPurged);
        };
        if suspended_at.is_none() {
            return Ok(AccountPurgeOutcome::Skipped);
        }
        let deletion_request = sqlx::query_as::<_, (i64, NaiveDateTime, bool)>(
            "SELECT id, created_at,
                    created_at <= clock_timestamp() - interval '30 days'
               FROM account_deletion_requests
              WHERE account_id = $1
              ORDER BY id LIMIT 1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if expected_deletion_request_id.is_some_and(|expected| {
            deletion_request
                .as_ref()
                .is_some_and(|(request_id, _, _)| *request_id != expected)
        }) || (expected_deletion_request_id.is_none()
            && expected_deletion_created_at.is_some_and(|expected| {
                deletion_request
                    .as_ref()
                    .is_some_and(|(_, created_at, _)| *created_at != expected)
            }))
        {
            return Ok(AccountPurgeOutcome::Skipped);
        }
        let Some((_, _, due)) = deletion_request else {
            return Ok(AccountPurgeOutcome::AlreadyPurged);
        };
        if !due {
            return Ok(AccountPurgeOutcome::Skipped);
        }
        if domain.is_some() && origin.is_none() {
            return Err(WriteError::InvalidInput(
                "remote account purge requires the instance origin",
            ));
        }

        if let Some(origin) = origin {
            reject_remote_account_follows(&mut transaction, account_id, origin).await?;
            undo_remote_account_follows(&mut transaction, account_id, origin).await?;
        }
        let protected_status_ids = protected_status_ids(&mut transaction, account_id).await?;
        purge_account_user(&mut transaction, account_id).await?;
        purge_account_profile(&mut transaction, account_id).await?;
        purge_account_statuses(
            &mut transaction,
            &mut pending_stream_events,
            account_id,
            &protected_status_ids,
            true,
        )
        .await?;
        purge_account_mentions(&mut transaction, account_id, &protected_status_ids).await?;
        purge_account_media(&mut transaction, account_id, &protected_status_ids).await?;
        purge_account_relationships(&mut transaction, account_id).await?;
        purge_account_notifications(&mut transaction, account_id).await?;
        purge_account_associations(&mut transaction, account_id).await?;
        ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        sqlx::query(
            "UPDATE account_stats SET statuses_count = 0, following_count = 0,
                followers_count = 0, last_status_at = NULL, updated_at = clock_timestamp()
              WHERE account_id = $1",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM account_deletion_requests WHERE account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        flush_staged_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(AccountPurgeOutcome::Purged)
    }

    /// Collects the Paperclip metadata for the current due account purge.
    ///
    /// The caller must hold the account lifecycle lock until the returned paths have been
    /// persisted and the purge transaction has committed.
    ///
    /// # Errors
    ///
    /// Returns a database error when the account or media metadata cannot be inspected.
    pub(crate) async fn account_purge_media_metadata(
        &self,
        account_id: i64,
        expected_deletion_request_id: Option<i64>,
        expected_deletion_created_at: Option<NaiveDateTime>,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let Some((domain, suspended_at)) =
            sqlx::query_as::<_, (Option<String>, Option<NaiveDateTime>)>(
                "SELECT domain, suspended_at FROM accounts WHERE id = $1 AND id <> -99 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Ok(Vec::new());
        };
        if suspended_at.is_none() {
            return Ok(Vec::new());
        }
        let deletion_request = sqlx::query_as::<_, (i64, NaiveDateTime, bool)>(
            "SELECT id, created_at,
                    created_at <= clock_timestamp() - interval '30 days'
               FROM account_deletion_requests
              WHERE account_id = $1
              ORDER BY id LIMIT 1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if expected_deletion_request_id.is_some_and(|expected| {
            deletion_request
                .as_ref()
                .is_some_and(|(request_id, _, _)| *request_id != expected)
        }) || (expected_deletion_request_id.is_none()
            && expected_deletion_created_at.is_some_and(|expected| {
                deletion_request
                    .as_ref()
                    .is_some_and(|(_, created_at, _)| *created_at != expected)
            }))
        {
            return Ok(Vec::new());
        }
        let Some((_, _, due)) = deletion_request else {
            return Ok(Vec::new());
        };
        if !due {
            return Ok(Vec::new());
        }
        let protected_status_ids = protected_status_ids(&mut transaction, account_id).await?;
        let metadata = account_media_metadata_for_cleanup(
            &mut transaction,
            account_id,
            domain.is_some(),
            &protected_status_ids,
        )
        .await?;
        transaction.commit().await?;
        Ok(metadata)
    }

    /// Creates or updates a global domain block for an authorized federation moderator.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account lacks `manage_federation`,
    /// [`WriteError::InvalidInput`] for an invalid domain or severity, or a database error when
    /// the block and audit record cannot commit atomically.
    #[allow(clippy::too_many_lines)]
    pub async fn set_domain_block(
        &self,
        acting_account_id: i64,
        domain: &str,
        severity: i32,
        reject_media: bool,
        reject_reports: bool,
        origin: &str,
    ) -> Result<i64, WriteError> {
        if !matches!(severity, 0..=2) {
            return Err(WriteError::InvalidInput("domain block severity is invalid"));
        }
        let domain = normalize_domain_block_domain(domain)?;
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        if authorized_admin_account(&mut transaction, acting_account_id, 1_i64 << 5)
            .await?
            .is_none()
        {
            return Err(WriteError::Unauthorized);
        }
        lock_domain_scope(&mut transaction, &domain).await?;
        let existing = sqlx::query_as::<_, (i64, String, Option<i32>, bool, bool, NaiveDateTime)>(
            "SELECT id, domain, severity, reject_media, reject_reports, created_at \
             FROM domain_blocks \
             WHERE lower(domain) = lower($1) OR lower($1) LIKE '%.' || lower(domain) \
             ORDER BY length(domain) DESC, id DESC FOR UPDATE",
        )
        .bind(&domain)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some((_, _, existing_severity, existing_reject_media, existing_reject_reports, _)) =
            existing.as_ref()
            && !domain_block_is_stricter(
                severity,
                reject_media,
                reject_reports,
                *existing_severity,
                *existing_reject_media,
                *existing_reject_reports,
            )
        {
            return Err(WriteError::Validation(
                "domain block would downgrade an existing rule",
            ));
        }
        let exact_existing = existing.filter(|(_, existing_domain, _, _, _, _)| {
            existing_domain.eq_ignore_ascii_case(&domain)
        });
        let policy_account_ids = if matches!(severity, 0 | 1) {
            let policy_domain = domain_policy_hostname(&domain);
            sqlx::query_scalar::<_, i64>(
                "SELECT id FROM accounts WHERE domain IS NOT NULL \
                   AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                            THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
                     OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                            THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
                 ORDER BY id",
            )
            .bind(&policy_domain)
            .fetch_all(&mut *transaction)
            .await?
        } else {
            Vec::new()
        };
        let policy_status_ids =
            timeline_status_ids_for_accounts(&mut transaction, &policy_account_ids).await?;
        let policy_before = status_timeline_snapshots(&mut transaction, &policy_status_ids).await?;
        let (domain_block_id, created_at, updated_at, action) = if let Some((
            id,
            _,
            _,
            _,
            _,
            created_at,
        )) = exact_existing
        {
            if severity != 1 {
                clear_domain_owned_account_restrictions(&mut transaction, &domain, created_at)
                    .await?;
            }
            let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
                "UPDATE domain_blocks SET severity = $2, reject_media = $3, reject_reports = $4, \
                    updated_at = clock_timestamp() WHERE id = $1 RETURNING updated_at",
            )
            .bind(id)
            .bind(severity)
            .bind(reject_media)
            .bind(reject_reports)
            .fetch_one(&mut *transaction)
            .await?;
            (id, created_at, updated_at, "update")
        } else {
            let (id, created_at, updated_at) = sqlx::query_as::<_, (i64, NaiveDateTime, NaiveDateTime)>(
                "INSERT INTO domain_blocks (domain, severity, reject_media, reject_reports, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp()) RETURNING id, created_at, updated_at",
            )
            .bind(&domain)
            .bind(severity)
            .bind(reject_media)
            .bind(reject_reports)
            .fetch_one(&mut *transaction)
            .await?;
            (id, created_at, updated_at, "create")
        };
        apply_domain_account_restrictions(&mut transaction, &domain, severity, created_at).await?;
        if !policy_status_ids.is_empty() {
            let policy_after =
                status_timeline_snapshots(&mut transaction, &policy_status_ids).await?;
            collect_timeline_snapshot_transitions(
                &mut transaction,
                &mut pending_stream_events,
                "status.update",
                &format!("domain-block:{domain_block_id}"),
                updated_at.and_utc().timestamp_micros(),
                &policy_before,
                &policy_after,
            )
            .await?;
        }
        let severance_event_id = if severity == 1 {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO relationship_severance_events
                         (type, target_name, purged, created_at, updated_at)
                     VALUES (0, $1, false, clock_timestamp(), clock_timestamp())
                     RETURNING id",
                )
                .bind(&domain)
                .fetch_one(&mut *transaction)
                .await?,
            )
        } else {
            None
        };
        if severity == 1 || reject_media {
            record_outbox_in(
                &mut transaction,
                &domain_block_job(domain_block_id, severance_event_id, updated_at, origin),
            )
            .await?;
        }
        insert_admin_action_log(
            &mut transaction,
            acting_account_id,
            action,
            domain_block_id,
            "DomainBlock",
            domain.clone(),
            None,
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(domain_block_id)
    }

    /// Queues a full purge of remote accounts and custom emoji for a domain.
    ///
    /// The request is durable and idempotent by domain. The maintenance worker performs the
    /// destructive work separately so the administrative command does not hold a public-schema
    /// transaction while removing potentially large remote datasets.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account lacks `manage_federation`,
    /// [`WriteError::InvalidInput`] for an invalid domain, or a database error when the outbox and
    /// audit record cannot commit atomically.
    pub async fn request_domain_purge(
        &self,
        acting_account_id: i64,
        domain: &str,
    ) -> Result<(), WriteError> {
        let domain = normalize_domain_block_domain(domain)?;
        let mut transaction = self.pool.begin().await?;
        if authorized_admin_account(&mut transaction, acting_account_id, 1_i64 << 5)
            .await?
            .is_none()
        {
            return Err(WriteError::Unauthorized);
        }
        lock_domain_scope(&mut transaction, &domain).await?;
        record_outbox_in(&mut transaction, &domain_purge_job(&domain)).await?;
        insert_instance_admin_action_log(&mut transaction, acting_account_id, &domain).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Permanently removes remote accounts and custom emoji for one domain.
    ///
    /// This is the worker-side equivalent of Rails `PurgeDomainService`: it does not retain remote
    /// account tombstones or emit `ActivityPub` side effects. The domain lock and idempotent deletes
    /// make a retried maintenance job safe.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::InvalidInput`] for an invalid domain or a database error when cleanup
    /// cannot commit atomically.
    pub(crate) async fn process_domain_purge_job(&self, domain: &str) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        sqlx::query(
            "UPDATE relationship_severance_events
                 SET purged = true, updated_at = clock_timestamp()
               WHERE type IN (0, 1) AND target_name = $1",
        )
        .bind(domain)
        .execute(&mut *transaction)
        .await?;
        let account_ids = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM accounts
               WHERE domain IS NOT NULL AND lower(domain) = lower($1)
               ORDER BY id FOR UPDATE",
        )
        .bind(domain)
        .fetch_all(&mut *transaction)
        .await?;
        for account_id in account_ids {
            purge_remote_account(&mut transaction, &mut pending_stream_events, account_id).await?;
        }
        sqlx::query(
            "DELETE FROM custom_emojis WHERE domain IS NOT NULL AND lower(domain) = lower($1)",
        )
        .bind(domain)
        .execute(&mut *transaction)
        .await?;
        flush_staged_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        sqlx::query("SELECT public.rustodon_refresh_instances()")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Purges remote accounts newly suspended by a global suspend-level domain block.
    ///
    /// # Errors
    ///
    /// Returns a database error when the block or any account purge cannot be read or committed.
    pub(crate) async fn process_domain_block_job(
        &self,
        domain_block_id: i64,
        severance_event_id: Option<i64>,
        origin: &str,
    ) -> Result<(), WriteError> {
        if origin.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "domain block processing requires the instance origin",
            ));
        }
        let domain =
            sqlx::query_scalar::<_, String>("SELECT domain FROM domain_blocks WHERE id = $1")
                .bind(domain_block_id)
                .fetch_optional(&self.pool)
                .await?;
        if let Some(domain) = domain {
            return self
                .with_domain_lock(&domain, || async {
                    self.process_domain_block_job_locked(
                        domain_block_id,
                        severance_event_id,
                        origin,
                    )
                    .await
                })
                .await;
        }
        self.process_domain_block_job_locked(domain_block_id, severance_event_id, origin)
            .await
    }

    pub(crate) async fn domain_block_domain(
        &self,
        domain_block_id: i64,
    ) -> Result<Option<String>, WriteError> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT domain FROM domain_blocks WHERE id = $1")
                .bind(domain_block_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub(crate) async fn process_domain_block_job_locked(
        &self,
        domain_block_id: i64,
        severance_event_id: Option<i64>,
        origin: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let Some((domain, severity, reject_media, created_at)) =
            sqlx::query_as::<_, (String, Option<i32>, bool, NaiveDateTime)>(
                "SELECT domain, severity, reject_media, created_at
                   FROM domain_blocks WHERE id = $1 FOR UPDATE",
            )
            .bind(domain_block_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            if let Some(severance_event_id) = severance_event_id {
                sqlx::query(
                    "UPDATE relationship_severance_events
                        SET purged = true, updated_at = clock_timestamp()
                      WHERE id = $1
                        AND NOT EXISTS (
                            SELECT 1 FROM severed_relationships
                             WHERE relationship_severance_event_id = $1
                        )",
                )
                .bind(severance_event_id)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
            return Ok(());
        };
        if severity != Some(1) {
            if reject_media {
                clear_domain_media(&mut transaction, &domain).await?;
            }
            if let Some(severance_event_id) = severance_event_id {
                sqlx::query(
                    "UPDATE relationship_severance_events
                        SET purged = true, updated_at = clock_timestamp()
                      WHERE id = $1",
                )
                .bind(severance_event_id)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
            return Ok(());
        }
        let Some(severance_event_id) = severance_event_id else {
            return Err(WriteError::InvalidInput(
                "suspend domain blocks require a severance event",
            ));
        };
        let domain = domain_policy_hostname(&domain);
        let accounts = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, uri FROM accounts
              WHERE domain IS NOT NULL
                AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                         THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1)
                  OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                         THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1))
                AND suspended_at = $2
              ORDER BY id",
        )
        .bind(&domain)
        .bind(created_at)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        for (account_id, actor_uri) in accounts {
            if !actor_uri.is_empty() {
                self.apply_remote_actor_delete(
                    account_id,
                    &actor_uri,
                    origin,
                    Some(severance_event_id),
                    Some(created_at),
                )
                .await?;
            }
        }
        let mut cleanup_transaction = self.pool.begin().await?;
        clear_domain_media(&mut cleanup_transaction, &domain).await?;
        cleanup_transaction.commit().await?;
        for (account_id, event_id) in self
            .create_domain_severance_events(severance_event_id)
            .await?
        {
            self.create_notification(NotificationCreate {
                recipient_account_id: account_id,
                activity: NotificationActivity::SeveredRelationships { id: event_id },
                silenced: false,
            })
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn domain_purge_media_metadata(
        &self,
        domain: &str,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let domain = normalize_domain_block_domain(domain)?;
        domain_media_metadata(&self.pool, &domain, false).await
    }

    pub(crate) async fn remote_actor_media_metadata(
        &self,
        account_id: i64,
        actor_uri: &str,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let Some((domain, current_uri)) = sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT domain, uri FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(Vec::new());
        };
        if domain.is_none() || current_uri != actor_uri {
            return Ok(Vec::new());
        }
        let metadata =
            account_media_metadata_for_cleanup(&mut transaction, account_id, true, &[]).await?;
        transaction.commit().await?;
        Ok(metadata)
    }

    pub(crate) async fn domain_block_media_metadata(
        &self,
        domain_block_id: i64,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let Some((domain, severity, reject_media)) =
            sqlx::query_as::<_, (String, Option<i32>, bool)>(
                "SELECT domain, severity, reject_media FROM domain_blocks WHERE id = $1",
            )
            .bind(domain_block_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(Vec::new());
        };
        if severity != Some(1) && !reject_media {
            return Ok(Vec::new());
        }
        domain_media_metadata(&self.pool, &domain_policy_hostname(&domain), true).await
    }

    pub(crate) async fn remote_note_media_metadata(
        &self,
        account_id: i64,
        actor_uri: &str,
        object_uri: &str,
        atom_uri: Option<&str>,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_remote_note(&mut transaction, object_uri).await?;
        let account = sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT domain, uri FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_actor_uri)) = account else {
            return Ok(Vec::new());
        };
        if domain.is_none() || current_actor_uri != actor_uri {
            return Ok(Vec::new());
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
        let Some(status_id) =
            remote_note_status_id_for_account(&mut transaction, account_id, object_uri, atom_uri)
                .await?
        else {
            transaction.commit().await?;
            return Ok(Vec::new());
        };
        let metadata = sqlx::query_as::<
            _,
            (
                i64,
                Option<i32>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<i32>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT id, file_storage_schema_version, file_file_name, file_content_type,
                    remote_url, thumbnail_storage_schema_version, thumbnail_file_name,
                    thumbnail_content_type, thumbnail_remote_url
               FROM media_attachments WHERE status_id = $1 ORDER BY id",
        )
        .bind(status_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let mut metadata_result = Vec::new();
        for (
            media_id,
            file_storage_schema_version,
            file_file_name,
            file_content_type,
            remote_url,
            thumbnail_storage_schema_version,
            thumbnail_file_name,
            thumbnail_content_type,
            thumbnail_remote_url,
        ) in metadata
        {
            append_media_attachment_metadata(
                &mut metadata_result,
                media_id,
                file_storage_schema_version,
                file_file_name,
                file_content_type,
                remote_url,
                thumbnail_storage_schema_version,
                thumbnail_file_name,
                thumbnail_content_type,
                thumbnail_remote_url,
            );
        }
        Ok(metadata_result)
    }

    pub(super) async fn create_domain_severance_events(
        &self,
        severance_event_id: i64,
    ) -> Result<Vec<(i64, i64)>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let events = sqlx::query_as::<_, (i64, i64)>(
            "INSERT INTO account_relationship_severance_events
                 (account_id, relationship_severance_event_id, followers_count,
                  following_count, created_at, updated_at)
             SELECT local_account_id, $1,
                    count(*) FILTER (WHERE direction = 0)::integer,
                    count(*) FILTER (WHERE direction = 1)::integer,
                    clock_timestamp(), clock_timestamp()
               FROM severed_relationships
              WHERE relationship_severance_event_id = $1
              GROUP BY local_account_id
             ON CONFLICT (account_id, relationship_severance_event_id) DO UPDATE
                 SET followers_count = EXCLUDED.followers_count,
                     following_count = EXCLUDED.following_count,
                     updated_at = clock_timestamp()
             RETURNING account_id, id",
        )
        .bind(severance_event_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(events)
    }

    /// Removes a global domain block for an authorized federation moderator.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account lacks `manage_federation`,
    /// [`WriteError::NotFound`] when the domain has no block, or a database error when the block
    /// and audit record cannot commit atomically.
    pub async fn unblock_domain(
        &self,
        acting_account_id: i64,
        domain: &str,
    ) -> Result<(), WriteError> {
        let domain = normalize_domain_block_domain(domain)?;
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        if authorized_admin_account(&mut transaction, acting_account_id, 1_i64 << 5)
            .await?
            .is_none()
        {
            return Err(WriteError::Unauthorized);
        }
        lock_domain_scope(&mut transaction, &domain).await?;
        let (domain_block_id, created_at) = sqlx::query_as::<_, (i64, NaiveDateTime)>(
            "SELECT id, created_at FROM domain_blocks \
             WHERE lower(domain) = lower($1) FOR UPDATE",
        )
        .bind(&domain)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let policy_domain = domain_policy_hostname(&domain);
        let account_ids = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM accounts WHERE domain IS NOT NULL \
               AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                        THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
                 OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                        THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
               AND (silenced_at = $2 OR suspended_at = $2) ORDER BY id",
        )
        .bind(&policy_domain)
        .bind(created_at)
        .fetch_all(&mut *transaction)
        .await?;
        let status_ids = timeline_status_ids_for_accounts(&mut transaction, &account_ids).await?;
        let before = status_timeline_snapshots(&mut transaction, &status_ids).await?;
        clear_domain_owned_account_restrictions(&mut transaction, &domain, created_at).await?;
        let after = status_timeline_snapshots(&mut transaction, &status_ids).await?;
        let transition_at =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?;
        collect_timeline_snapshot_transitions(
            &mut transaction,
            &mut pending_stream_events,
            "status.update",
            &format!("domain-unblock:{domain_block_id}"),
            transition_at.and_utc().timestamp_micros(),
            &before,
            &after,
        )
        .await?;
        sqlx::query("DELETE FROM domain_blocks WHERE id = $1")
            .bind(domain_block_id)
            .execute(&mut *transaction)
            .await?;
        insert_admin_action_log(
            &mut transaction,
            acting_account_id,
            "destroy",
            domain_block_id,
            "DomainBlock",
            domain,
            None,
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }
}
