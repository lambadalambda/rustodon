//! `OAuth` applications and tokens, browser authentication, passwords, users and sessions.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

#[allow(clippy::missing_errors_doc)]
impl WriteRepository {
    pub async fn register_oauth_application(
        &self,
        registration: &OAuthApplicationRegistration,
    ) -> Result<OAuthApplicationRegistrationResult, WriteError> {
        if validate_oauth_application_registration(registration).is_err() {
            return Err(WriteError::Validation("Validation failed"));
        }
        let uid = random_urlsafe_base64(32);
        let client_secret = random_urlsafe_base64(32);
        let scopes = canonical_oauth_scopes(&registration.scopes);
        let mut transaction = self.pool.begin().await?;
        let application = sqlx::query_as::<_, crate::mastodon::records::OAuthApplication>(
            "INSERT INTO oauth_applications ( \
               name, uid, secret, redirect_uri, scopes, confidential, website, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, true, $6, clock_timestamp(), clock_timestamp()) \
             RETURNING id, name, uid, secret, redirect_uri, scopes, confidential, owner_id, owner_type, website",
        )
        .bind(&registration.name)
        .bind(&uid)
        .bind(&client_secret)
        .bind(&registration.redirect_uri)
        .bind(&scopes)
        .bind(&registration.website)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OAuthApplicationRegistrationResult {
            application,
            client_secret,
        })
    }

    pub async fn create_oauth_authorization_grant(
        &self,
        client_id: &str,
        resource_owner_id: i64,
        redirect_uri: &str,
        requested_scopes: Option<&str>,
        code_challenge: Option<&str>,
        code_challenge_method: Option<&str>,
    ) -> Result<OAuthAuthorizationGrant, OAuthAuthorizationGrantError> {
        let mut transaction = self.pool.begin().await?;
        let Some((application_id, application_redirect_uri, application_scopes, confidential)) =
            sqlx::query_as::<_, (i64, String, String, bool)>(
                "SELECT id, redirect_uri, scopes, confidential \
                 FROM oauth_applications WHERE uid = $1",
            )
            .bind(client_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(OAuthAuthorizationGrantError::InvalidClient);
        };
        if !application_redirect_uri
            .split_whitespace()
            .any(|registered| registered == redirect_uri)
        {
            return Err(OAuthAuthorizationGrantError::InvalidRedirectUri);
        }
        if !oauth_grant_pkce_is_valid(confidential, code_challenge, code_challenge_method) {
            return Err(OAuthAuthorizationGrantError::InvalidCodeChallenge);
        }
        let scopes = requested_scopes
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| "read".to_owned(), canonical_oauth_scopes);
        let application_scopes = application_scopes.split_whitespace().collect::<Vec<_>>();
        if scopes.split_whitespace().any(|scope| {
            !application_scopes.contains(&scope) || !OAUTH_CONFIGURED_SCOPES.contains(&scope)
        }) {
            return Err(OAuthAuthorizationGrantError::InvalidScope);
        }
        let code = random_urlsafe_base64(32);
        let (code, scopes) = sqlx::query_as::<_, (String, String)>(
            "INSERT INTO oauth_access_grants ( \
                application_id, code_challenge, code_challenge_method, created_at, expires_in, \
                redirect_uri, resource_owner_id, revoked_at, scopes, token) \
             VALUES ($1, $2, $3, clock_timestamp(), 600, $4, $5, NULL, $6, $7) \
             RETURNING token, scopes",
        )
        .bind(application_id)
        .bind(code_challenge)
        .bind(code_challenge_method)
        .bind(redirect_uri)
        .bind(resource_owner_id)
        .bind(&scopes)
        .bind(&code)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OAuthAuthorizationGrant { code, scopes })
    }

    pub async fn issue_oauth_client_credentials_token(
        &self,
        client_id: &str,
        client_secret: &str,
        requested_scopes: Option<&str>,
    ) -> Result<OAuthClientCredentialsToken, OAuthClientCredentialsError> {
        if client_id.trim().is_empty() || client_secret.is_empty() {
            return Err(OAuthClientCredentialsError::InvalidClient);
        }
        let mut transaction = self.pool.begin().await?;
        let Some((application_id, stored_secret, application_scopes, confidential)) =
            sqlx::query_as::<_, (i64, String, String, bool)>(
                "SELECT id, secret, scopes, confidential \
                 FROM oauth_applications WHERE uid = $1",
            )
            .bind(client_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(OAuthClientCredentialsError::InvalidClient);
        };
        if !confidential || !constant_time_string_equal(&stored_secret, client_secret) {
            return Err(OAuthClientCredentialsError::InvalidClient);
        }
        let scopes = requested_scopes
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| "read".to_owned(), canonical_oauth_scopes);
        let application_scopes = application_scopes.split_whitespace().collect::<Vec<_>>();
        if scopes.split_whitespace().any(|scope| {
            !application_scopes.contains(&scope) || !OAUTH_CONFIGURED_SCOPES.contains(&scope)
        }) {
            return Err(OAuthClientCredentialsError::InvalidScope);
        }
        if let Some((access_token, created_at)) = sqlx::query_as::<_, (String, NaiveDateTime)>(
            "SELECT token, created_at FROM oauth_access_tokens \
             WHERE application_id = $1 AND resource_owner_id IS NULL \
               AND scopes = $2 AND revoked_at IS NULL \
               AND (expires_in IS NULL OR created_at + expires_in * INTERVAL '1 second' > clock_timestamp()) \
             ORDER BY id LIMIT 1 FOR UPDATE",
        )
        .bind(application_id)
        .bind(&scopes)
        .fetch_optional(&mut *transaction)
        .await?
        {
            transaction.commit().await?;
            return Ok(OAuthClientCredentialsToken {
                access_token,
                scopes,
                created_at,
            });
        }
        let access_token = random_urlsafe_base64(32);
        let (access_token, created_at) = sqlx::query_as::<_, (String, NaiveDateTime)>(
            "INSERT INTO oauth_access_tokens ( \
               resource_owner_id, application_id, token, refresh_token, scopes, \
               expires_in, created_at, revoked_at, last_used_at, last_used_ip) \
             VALUES (NULL, $1, $2, NULL, $3, NULL, clock_timestamp(), NULL, NULL, NULL) \
             RETURNING token, created_at",
        )
        .bind(application_id)
        .bind(access_token)
        .bind(&scopes)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OAuthClientCredentialsToken {
            access_token,
            scopes,
            created_at,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn issue_oauth_authorization_code_token(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> Result<OAuthAuthorizationCodeToken, OAuthAuthorizationCodeError> {
        if client_id.trim().is_empty() || code.is_empty() || redirect_uri.is_empty() {
            return Err(OAuthAuthorizationCodeError::InvalidGrant);
        }
        let mut transaction = self.pool.begin().await?;
        let Some((
            grant_id,
            application_id,
            stored_secret,
            confidential,
            grant_challenge,
            grant_challenge_method,
            grant_expires_in,
            grant_created_at,
            grant_redirect_uri,
            resource_owner_id,
            grant_revoked_at,
            grant_scopes,
        )) = sqlx::query_as::<
            _,
            (
                i64,
                i64,
                String,
                bool,
                Option<String>,
                Option<String>,
                i32,
                NaiveDateTime,
                String,
                i64,
                Option<NaiveDateTime>,
                Option<String>,
            ),
        >(
            "SELECT access_grant.id, access_grant.application_id, application.secret, \
                    application.confidential, access_grant.code_challenge, \
                    access_grant.code_challenge_method, access_grant.expires_in, \
                    access_grant.created_at, access_grant.redirect_uri, \
                    access_grant.resource_owner_id, access_grant.revoked_at, access_grant.scopes \
             FROM oauth_access_grants access_grant \
             JOIN oauth_applications application ON application.id = access_grant.application_id \
             WHERE application.uid = $1 AND access_grant.token = $2 \
             FOR UPDATE OF access_grant",
        )
        .bind(client_id)
        .bind(code)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Err(OAuthAuthorizationCodeError::InvalidGrant);
        };
        if confidential
            && !constant_time_string_equal(&stored_secret, client_secret.unwrap_or_default())
        {
            return Err(OAuthAuthorizationCodeError::InvalidClient);
        }
        if grant_redirect_uri != redirect_uri
            || grant_revoked_at.is_some()
            || grant_created_at
                .checked_add_signed(ChronoDuration::seconds(i64::from(grant_expires_in)))
                .is_none_or(|expires_at| expires_at <= Utc::now().naive_utc())
            || resource_owner_id <= 0
        {
            return Err(OAuthAuthorizationCodeError::InvalidGrant);
        }
        if !oauth_pkce_matches(
            confidential,
            grant_challenge.as_deref(),
            grant_challenge_method.as_deref(),
            code_verifier,
        ) {
            return Err(OAuthAuthorizationCodeError::InvalidGrant);
        }
        sqlx::query("UPDATE oauth_access_grants SET revoked_at = clock_timestamp() WHERE id = $1")
            .bind(grant_id)
            .execute(&mut *transaction)
            .await?;
        let scopes = grant_scopes.unwrap_or_default();
        let access_token = random_urlsafe_base64(32);
        let (access_token, created_at) = sqlx::query_as::<_, (String, NaiveDateTime)>(
            "INSERT INTO oauth_access_tokens ( \
                resource_owner_id, application_id, token, refresh_token, scopes, expires_in, \
                created_at, revoked_at, last_used_at, last_used_ip) \
             VALUES ($1, $2, $3, NULL, $4, NULL, clock_timestamp(), NULL, NULL, NULL) \
             RETURNING token, created_at",
        )
        .bind(resource_owner_id)
        .bind(application_id)
        .bind(access_token)
        .bind(&scopes)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OAuthAuthorizationCodeToken {
            access_token,
            scopes,
            created_at,
        })
    }

    /// Authenticates a persisted Mastodon login and records the result.
    ///
    /// # Errors
    ///
    /// Returns a stable authentication failure or a database error.
    ///
    /// # Panics
    ///
    /// Panics only if the current Unix TOTP timestep cannot fit the persisted
    /// `PostgreSQL` integer column.
    #[allow(clippy::too_many_lines)]
    pub async fn authenticate_browser_user(
        &self,
        email: &str,
        password: &str,
        otp_attempt: Option<&str>,
        timestamp: i64,
        ip: IpNetwork,
        user_agent: &str,
    ) -> Result<BrowserAuthentication, BrowserAuthenticationError> {
        let mut transaction = self.pool.begin().await?;
        let user = sqlx::query_as::<_, BrowserLoginUser>(
            "SELECT u.id, u.account_id, u.encrypted_password, \
                    u.otp_backup_codes::text[] AS otp_backup_codes, \
                    u.otp_required_for_login, u.otp_secret, u.consumed_timestep, \
                    u.approved, u.confirmed_at, \
                    COALESCE(role.require_2fa, false) AS role_requires_2fa, \
                    EXISTS (SELECT 1 FROM webauthn_credentials credential \
                            WHERE credential.user_id = u.id) AS has_webauthn_credentials, \
                    account.memorial AS account_memorial \
             FROM users u \
             JOIN accounts account ON account.id = u.account_id \
             LEFT JOIN user_roles role ON role.id = u.role_id \
             WHERE lower(u.email) = lower($1) \
             FOR UPDATE OF u",
        )
        .bind(email)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(mut user) = user else {
            let _ = verify_password(password, DUMMY_BCRYPT_PASSWORD);
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::InvalidCredentials);
        };
        if !verify_password(password, user.encrypted_password.as_str()) {
            record_login_activity(
                &mut transaction,
                user.id,
                "password",
                Some("invalid_password"),
                false,
                ip,
                user_agent,
            )
            .await?;
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::InvalidCredentials);
        }
        if user.confirmed_at.is_none() {
            record_login_activity(
                &mut transaction,
                user.id,
                "password",
                Some("unconfirmed"),
                false,
                ip,
                user_agent,
            )
            .await?;
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::Unconfirmed);
        }
        if !user.approved {
            record_login_activity(
                &mut transaction,
                user.id,
                "password",
                Some("pending_approval"),
                false,
                ip,
                user_agent,
            )
            .await?;
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::PendingApproval);
        }
        if user.account_memorial {
            record_login_activity(
                &mut transaction,
                user.id,
                "password",
                Some("inactive"),
                false,
                ip,
                user_agent,
            )
            .await?;
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::Memorialized);
        }

        let otp_secret = self.decrypt_otp_secret(user.otp_secret.take())?;
        let requires_two_factor = user.otp_required_for_login || user.has_webauthn_credentials;
        let (method, two_factor_update) = if requires_two_factor {
            let Some(otp_attempt) = otp_attempt.filter(|attempt| !attempt.trim().is_empty()) else {
                transaction.commit().await?;
                return Err(BrowserAuthenticationError::TwoFactorRequired);
            };
            let failures = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM login_activities activity \
                 WHERE activity.user_id = $1 AND activity.success = false \
                   AND activity.authentication_method IN ('otp', 'backup_code') \
                   AND activity.created_at > clock_timestamp()::timestamp - INTERVAL '1 hour' \
                   AND activity.created_at > COALESCE( \
                       (SELECT MAX(success_activity.created_at) \
                          FROM login_activities success_activity \
                         WHERE success_activity.user_id = $1 \
                           AND success_activity.success = true), \
                       clock_timestamp()::timestamp - INTERVAL '1 hour')",
            )
            .bind(user.id)
            .fetch_one(&mut *transaction)
            .await?;
            if two_factor_attempt_is_rate_limited(failures) {
                transaction.commit().await?;
                return Err(BrowserAuthenticationError::RateLimited);
            }
            let backup_codes = user
                .otp_backup_codes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|code| code.as_str().to_owned())
                .collect::<Vec<_>>();
            match verify_two_factor(
                otp_secret
                    .as_ref()
                    .map(crate::mastodon::types::SecretText::as_str),
                &backup_codes,
                otp_attempt,
                timestamp,
                user.consumed_timestep.map(i64::from),
            ) {
                TwoFactorVerification::Totp(timestep) => (
                    BrowserAuthenticationMethod::Totp,
                    Some((
                        Some(
                            i32::try_from(timestep)
                                .expect("current TOTP timestep fits PostgreSQL integer"),
                        ),
                        None,
                    )),
                ),
                TwoFactorVerification::BackupCode(index) => {
                    let mut remaining = backup_codes;
                    remaining.remove(index);
                    (
                        BrowserAuthenticationMethod::BackupCode,
                        Some((None, Some(remaining))),
                    )
                }
                TwoFactorVerification::Invalid => {
                    record_login_activity(
                        &mut transaction,
                        user.id,
                        "otp",
                        Some("invalid_otp_token"),
                        false,
                        ip,
                        user_agent,
                    )
                    .await?;
                    transaction.commit().await?;
                    return Err(BrowserAuthenticationError::InvalidTwoFactor);
                }
            }
        } else {
            (BrowserAuthenticationMethod::Password, None)
        };

        if let Some((consumed_timestep, backup_codes)) = two_factor_update {
            if let Some(consumed_timestep) = consumed_timestep {
                sqlx::query(
                    "UPDATE users SET consumed_timestep = $1, updated_at = clock_timestamp() \
                     WHERE id = $2",
                )
                .bind(consumed_timestep)
                .bind(user.id)
                .execute(&mut *transaction)
                .await?;
            } else if let Some(backup_codes) = backup_codes {
                sqlx::query(
                    "UPDATE users SET otp_backup_codes = $1, updated_at = clock_timestamp() \
                     WHERE id = $2",
                )
                .bind(backup_codes)
                .bind(user.id)
                .execute(&mut *transaction)
                .await?;
            }
        }
        sqlx::query(
            "UPDATE users SET last_sign_in_at = current_sign_in_at, \
                    current_sign_in_at = clock_timestamp(), sign_in_count = sign_in_count + 1, \
                    updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
        record_login_activity(
            &mut transaction,
            user.id,
            match method {
                BrowserAuthenticationMethod::Password => "password",
                BrowserAuthenticationMethod::Totp | BrowserAuthenticationMethod::BackupCode => {
                    "otp"
                }
            },
            None,
            true,
            ip,
            user_agent,
        )
        .await?;
        transaction.commit().await?;
        Ok(BrowserAuthentication {
            user_id: user.id,
            account_id: user.account_id,
            method,
            password: VerifiedPassword {
                user_id: user.id,
                encrypted_password: user.encrypted_password,
            },
        })
    }

    /// Verifies a current password; authorized writes must fence this proof in their transaction.
    pub async fn verify_current_password(
        &self,
        user_id: i64,
        password: &str,
    ) -> Result<VerifiedPassword, WriteError> {
        let encrypted_password = sqlx::query_scalar::<_, crate::mastodon::types::SecretText>(
            "SELECT encrypted_password FROM users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(WriteError::Unauthorized)?;
        if !verify_password(password, encrypted_password.as_str()) {
            return Err(WriteError::Unauthorized);
        }
        Ok(VerifiedPassword {
            user_id,
            encrypted_password,
        })
    }

    /// Changes a password only if the checked credential is still current under the user lock.
    pub async fn change_user_password(
        &self,
        authentication: &VerifiedPassword,
        password: &str,
    ) -> Result<(), WriteError> {
        validate_local_password(password)?;
        let mut transaction = self.pool.begin().await?;
        let account_id = lock_verified_password_in(&mut transaction, authentication).await?;
        replace_user_password_in(
            &mut transaction,
            authentication.user_id,
            account_id,
            password,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Enables TOTP authentication and returns the one-time backup codes.
    ///
    /// The database stores only bcrypt hashes of the returned codes. The caller must
    /// present the codes to the user immediately because they cannot be recovered later.
    pub async fn enable_two_factor_authentication(
        &self,
        user_id: i64,
        otp_secret: &str,
    ) -> Result<Vec<String>, WriteError> {
        if user_id <= 0 || !valid_totp_secret(otp_secret) {
            return Err(WriteError::InvalidInput("invalid two-factor setup"));
        }
        let stored_otp_secret = self.encrypt_otp_secret(otp_secret)?;
        let mut transaction = self.pool.begin().await?;
        let enabled = sqlx::query_scalar::<_, bool>(
            "SELECT otp_required_for_login FROM users WHERE id = $1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if enabled {
            return Err(WriteError::Validation(
                "two-factor authentication is already enabled",
            ));
        }
        let (backup_codes, encrypted_backup_codes) = generate_backup_codes().await?;
        sqlx::query(
            "UPDATE users SET otp_secret = $1, otp_backup_codes = $2, \
                    otp_required_for_login = true, consumed_timestep = NULL, \
                    updated_at = clock_timestamp() WHERE id = $3",
        )
        .bind(stored_otp_secret)
        .bind(encrypted_backup_codes)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(backup_codes)
    }

    /// Disables TOTP authentication after verifying the user's current password.
    pub async fn disable_two_factor_authentication(
        &self,
        user_id: i64,
        current_password: &str,
    ) -> Result<(), WriteError> {
        if user_id <= 0 || current_password.is_empty() {
            return Err(WriteError::InvalidInput(
                "user ID and current password must not be empty",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let (encrypted_password, role_requires_2fa) = sqlx::query_as::<_, (String, bool)>(
            "SELECT user_record.encrypted_password, COALESCE(role.require_2fa, false) \
               FROM users user_record \
               LEFT JOIN user_roles role ON role.id = user_record.role_id \
              WHERE user_record.id = $1 \
              FOR UPDATE OF user_record",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if !verify_password(current_password, &encrypted_password) {
            return Err(WriteError::Unauthorized);
        }
        if role_requires_2fa {
            return Err(WriteError::Validation(
                "two-factor authentication is required by the user's role",
            ));
        }
        sqlx::query(
            "UPDATE users SET otp_required_for_login = false, otp_secret = NULL, \
                    otp_backup_codes = ARRAY[]::text[], consumed_timestep = NULL, \
                    updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM webauthn_credentials WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Replaces the stored backup codes after verifying the user's current password.
    pub async fn regenerate_two_factor_backup_codes(
        &self,
        user_id: i64,
        current_password: &str,
    ) -> Result<Vec<String>, WriteError> {
        if user_id <= 0 || current_password.is_empty() {
            return Err(WriteError::InvalidInput(
                "user ID and current password must not be empty",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let user = sqlx::query_as::<_, (String, bool)>(
            "SELECT encrypted_password, otp_required_for_login \
               FROM users WHERE id = $1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if !verify_password(current_password, &user.0) {
            return Err(WriteError::Unauthorized);
        }
        if !user.1 {
            return Err(WriteError::Validation(
                "two-factor authentication is not enabled",
            ));
        }
        let (backup_codes, encrypted_backup_codes) = generate_backup_codes().await?;
        sqlx::query(
            "UPDATE users SET otp_backup_codes = $1, updated_at = clock_timestamp() \
             WHERE id = $2",
        )
        .bind(encrypted_backup_codes)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(backup_codes)
    }

    /// Replaces an existing user's password and invalidates active sessions and tokens.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error when the password cannot be stored.
    pub async fn reset_user_password_by_email(
        &self,
        email: &str,
        password: &str,
    ) -> Result<bool, WriteError> {
        if email.trim().is_empty() || password.is_empty() {
            return Err(WriteError::InvalidInput(
                "email and password must not be empty",
            ));
        }
        validate_local_password(password)?;
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, account_id FROM users \
              WHERE lower(email) = lower($1) AND COALESCE(encrypted_password, '') <> '' \
              FOR UPDATE",
        )
        .bind(email)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((user_id, account_id)) = user_id else {
            transaction.commit().await?;
            return Ok(false);
        };
        replace_user_password_in(&mut transaction, user_id, account_id, password).await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Creates a single-use password-reset token for an existing user.
    ///
    /// The returned value is the only copy of the token. This compatibility
    /// helper stores the legacy SHA-256 digest.
    pub async fn create_password_reset_token(
        &self,
        email: &str,
    ) -> Result<Option<String>, WriteError> {
        if email.trim().is_empty() {
            return Err(WriteError::InvalidInput("email must not be empty"));
        }
        let token = random_urlsafe_base64(32);
        let digest = password_reset_digest(&token);
        let result = sqlx::query(
            "UPDATE users SET reset_password_token = $1, reset_password_sent_at = clock_timestamp(), \
                    updated_at = clock_timestamp() \
             WHERE lower(email) = lower($2) AND COALESCE(encrypted_password, '') <> ''",
        )
        .bind(digest)
        .bind(email)
        .execute(&self.pool)
        .await?;
        Ok((result.rows_affected() == 1).then_some(token))
    }

    /// Creates a password-reset token and records its mail job atomically.
    ///
    /// # Errors
    ///
    /// Returns a validation, job, or database error when the token or outbox event cannot be
    /// stored. The plaintext token is only passed to the in-memory job factory.
    pub async fn create_password_reset_token_with_job<F>(
        &self,
        email: &str,
        token_digest_secret: Option<&str>,
        job_factory: F,
    ) -> Result<Option<String>, WriteError>
    where
        F: FnOnce(&str) -> Result<JobSpec, WriteError>,
    {
        if email.trim().is_empty() {
            return Err(WriteError::InvalidInput("email must not be empty"));
        }
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM users \
             WHERE lower(email) = lower($1) AND COALESCE(encrypted_password, '') <> '' \
             FOR UPDATE",
        )
        .bind(email)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(user_id) = user_id else {
            transaction.commit().await?;
            return Ok(None);
        };
        let token = random_urlsafe_base64(32);
        let digest = devise_token_digest("reset_password_token", &token, token_digest_secret);
        let job = job_factory(&token)?;
        sqlx::query(
            "UPDATE users SET reset_password_token = $1, reset_password_sent_at = clock_timestamp(), \
                    updated_at = clock_timestamp() WHERE id = $2",
        )
        .bind(digest)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        record_outbox_in(&mut transaction, &job).await?;
        transaction.commit().await?;
        Ok(Some(token))
    }

    /// Creates a local user as a confirmed account without invoking registration or invites.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error when the account cannot be created.
    pub async fn create_local_user(
        &self,
        email: &str,
        username: &str,
        password: &str,
    ) -> Result<CreatedLocalUser, WriteError> {
        self.insert_local_user(email, username, password, None)
            .await
    }

    /// Creates an unconfirmed local user and records its confirmation mail atomically.
    ///
    /// # Errors
    ///
    /// Returns a validation, job, or database error when the account or confirmation event cannot
    /// be created.
    pub async fn create_local_user_with_confirmation(
        &self,
        email: &str,
        username: &str,
        password: &str,
        confirmation_token: &str,
        token_digest_secret: Option<&str>,
        confirmation_job: &JobSpec,
    ) -> Result<CreatedLocalUser, WriteError> {
        if confirmation_token.is_empty() {
            return Err(WriteError::InvalidInput(
                "confirmation token must not be empty",
            ));
        }
        self.insert_local_user(
            email,
            username,
            password,
            Some((confirmation_token, token_digest_secret, confirmation_job)),
        )
        .await
    }

    /// Confirms an account using a non-expired single-use confirmation token.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error when confirmation cannot be checked.
    pub async fn confirm_user_with_token(&self, token: &str) -> Result<bool, WriteError> {
        self.confirm_user_with_token_and_optional_secret(token, None)
            .await
    }

    /// Confirms an account using the Rails secret key base, with a legacy digest fallback.
    pub async fn confirm_user_with_token_and_secret(
        &self,
        token: &str,
        token_digest_secret: &str,
    ) -> Result<bool, WriteError> {
        self.confirm_user_with_token_and_optional_secret(token, Some(token_digest_secret))
            .await
    }

    pub(super) async fn confirm_user_with_token_and_optional_secret(
        &self,
        token: &str,
        token_digest_secret: Option<&str>,
    ) -> Result<bool, WriteError> {
        if token.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "confirmation token must not be empty",
            ));
        }
        let digest = devise_token_digest("confirmation_token", token, token_digest_secret);
        let legacy_digest = password_reset_digest(token);
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM users \
             WHERE confirmed_at IS NULL AND confirmation_token IN ($1, $2, $3) \
               AND confirmation_sent_at > clock_timestamp() - INTERVAL '2 days' \
             FOR UPDATE",
        )
        .bind(digest)
        .bind(legacy_digest)
        .bind(token)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(user_id) = user_id else {
            transaction.commit().await?;
            return Ok(false);
        };
        sqlx::query(
            "UPDATE users SET confirmed_at = clock_timestamp(), confirmation_token = NULL, \
                    confirmation_sent_at = NULL, updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        crate::activity::record_activation_in(&mut transaction, user_id, false).await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub(super) async fn insert_local_user(
        &self,
        email: &str,
        username: &str,
        password: &str,
        confirmation: Option<(&str, Option<&str>, &JobSpec)>,
    ) -> Result<CreatedLocalUser, WriteError> {
        let email = normalize_local_email(email)?;
        let username = normalize_local_username(username)?;
        validate_local_password(password)?;
        let encrypted_password =
            hash(password, DEFAULT_COST).map_err(|_| WriteError::Validation("invalid password"))?;
        let (private_key, public_key) = local_signing_keys()?;
        let (confirmed, confirmation_token) =
            confirmation.map_or((true, None), |(token, token_digest_secret, _)| {
                (
                    false,
                    Some(devise_token_digest(
                        "confirmation_token",
                        token,
                        token_digest_secret,
                    )),
                )
            });
        let mut transaction = self.pool.begin().await?;
        let account_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO accounts (username, private_key, public_key, created_at, updated_at) \
             VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(&username)
        .bind(private_key)
        .bind(public_key)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO account_stats (account_id, created_at, updated_at) \
             VALUES ($1, clock_timestamp(), clock_timestamp())",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        let user_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO users (account_id, email, encrypted_password, approved, confirmed_at, \
                    confirmation_token, confirmation_sent_at, created_at, updated_at) \
             VALUES ($1, $2, $3, true, CASE WHEN $4 THEN clock_timestamp() ELSE NULL END, \
                    $5, CASE WHEN $4 THEN NULL ELSE clock_timestamp() END, \
                    clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(account_id)
        .bind(&email)
        .bind(encrypted_password)
        .bind(confirmed)
        .bind(confirmation_token)
        .fetch_one(&mut *transaction)
        .await?;
        if let Some((_, _, confirmation_job)) = confirmation {
            record_outbox_in(&mut transaction, confirmation_job).await?;
        }
        crate::activity::record_activation_in(&mut transaction, user_id, false).await?;
        transaction.commit().await?;
        Ok(CreatedLocalUser {
            account_id,
            user_id,
            confirmed,
        })
    }

    /// Consumes a non-expired password-reset token and revokes prior access.
    pub async fn reset_password_with_token(
        &self,
        token: &str,
        password: &str,
    ) -> Result<bool, WriteError> {
        self.reset_password_with_token_and_optional_secret(token, password, None)
            .await
    }

    /// Consumes a password-reset token using the Rails secret key base, with a legacy digest fallback.
    pub async fn reset_password_with_token_and_secret(
        &self,
        token: &str,
        password: &str,
        token_digest_secret: &str,
    ) -> Result<bool, WriteError> {
        self.reset_password_with_token_and_optional_secret(
            token,
            password,
            Some(token_digest_secret),
        )
        .await
    }

    pub(super) async fn reset_password_with_token_and_optional_secret(
        &self,
        token: &str,
        password: &str,
        token_digest_secret: Option<&str>,
    ) -> Result<bool, WriteError> {
        if token.trim().is_empty() || password.is_empty() {
            return Err(WriteError::InvalidInput(
                "reset token and password must not be empty",
            ));
        }
        validate_local_password(password)?;
        let digest = devise_token_digest("reset_password_token", token, token_digest_secret);
        let legacy_digest = password_reset_digest(token);
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, account_id FROM users \
              WHERE reset_password_token IN ($1, $2) \
                AND COALESCE(encrypted_password, '') <> '' \
               AND reset_password_sent_at > clock_timestamp() - INTERVAL '6 hours' \
             FOR UPDATE",
        )
        .bind(digest)
        .bind(legacy_digest)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((user_id, account_id)) = user_id else {
            transaction.commit().await?;
            return Ok(false);
        };
        replace_user_password_in(&mut transaction, user_id, account_id, password).await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn create_browser_session(
        &self,
        authentication: &BrowserAuthentication,
        ip: IpNetwork,
        user_agent: &str,
    ) -> Result<String, WriteError> {
        let session_id = random_urlsafe_base64(32);
        let access_token = random_urlsafe_base64(32);
        let mut transaction = self.pool.begin().await?;
        lock_verified_password_in(&mut transaction, &authentication.password).await?;
        let user_id = authentication.password.user_id;
        let application_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM oauth_applications WHERE superapp = true ORDER BY id LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await?;
        let access_token_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO oauth_access_tokens ( \
                application_id, created_at, expires_in, last_used_at, last_used_ip, \
                refresh_token, resource_owner_id, revoked_at, scopes, token) \
             VALUES ($1, clock_timestamp(), NULL, NULL, NULL, NULL, $2, NULL, \
                     'read write follow', $3) \
             RETURNING id",
        )
        .bind(application_id)
        .bind(user_id)
        .bind(access_token)
        .fetch_one(&mut *transaction)
        .await?;
        let session_id = sqlx::query_scalar::<_, String>(
            "INSERT INTO session_activations ( \
                access_token_id, created_at, ip, session_id, updated_at, user_agent, user_id, \
                web_push_subscription_id) \
             VALUES ($1, clock_timestamp(), $2, $3, clock_timestamp(), $4, $5, NULL) \
             RETURNING session_id",
        )
        .bind(access_token_id)
        .bind(ip)
        .bind(session_id)
        .bind(user_agent)
        .bind(user_id)
        .fetch_one(&mut *transaction)
        .await?;
        crate::activity::record_login_in(&mut transaction, user_id).await?;
        transaction.commit().await?;
        Ok(session_id)
    }

    /// Track only explicit interactive routes, never generic bearer/media reads.
    pub async fn track_interactive_user(&self, user_id: i64) -> sqlx::Result<()> {
        let mut transaction = self.pool.begin().await?;
        crate::activity::track_returning_in(&mut transaction, user_id).await?;
        transaction.commit().await
    }

    pub async fn touch_browser_session(&self, session_id: &str) -> sqlx::Result<bool> {
        let result = sqlx::query(
            "UPDATE session_activations SET updated_at = clock_timestamp() \
             WHERE session_id = $1 AND updated_at > clock_timestamp() - INTERVAL '30 days'",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn delete_browser_session(&self, session_id: &str) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let token_row = sqlx::query_as::<_, (Option<i64>, Option<i64>, i64)>(
            "SELECT session.access_token_id, session.web_push_subscription_id, user_record.account_id
               FROM session_activations session
               JOIN users user_record ON user_record.id = session.user_id
              WHERE session.session_id = $1 FOR UPDATE",
        )
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM session_activations WHERE session_id = $1")
            .bind(session_id)
            .execute(&mut *transaction)
            .await?;
        if let Some((access_token_id, web_push_subscription_id, account_id)) = token_row {
            if let Some(web_push_subscription_id) = web_push_subscription_id {
                sqlx::query("DELETE FROM web_push_subscriptions WHERE id = $1")
                    .bind(web_push_subscription_id)
                    .execute(&mut *transaction)
                    .await?;
            }
            if let Some(access_token_id) = access_token_id {
                sqlx::query("DELETE FROM oauth_access_tokens WHERE id = $1")
                    .bind(access_token_id)
                    .execute(&mut *transaction)
                    .await?;
                record_token_kill_stream_event(&mut transaction, account_id, access_token_id)
                    .await?;
            }
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_oauth_token(
        &self,
        client_id: &str,
        client_secret: &str,
        token: Option<&str>,
        token_type_hint: Option<&str>,
    ) -> Result<(), OAuthTokenRevocationError> {
        if client_id.trim().is_empty() {
            return Err(OAuthTokenRevocationError::InvalidClient);
        }
        let mut transaction = self.pool.begin().await?;
        let Some((application_id, stored_secret, confidential)) =
            sqlx::query_as::<_, (i64, String, bool)>(
                "SELECT id, secret, confidential FROM oauth_applications WHERE uid = $1",
            )
            .bind(client_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(OAuthTokenRevocationError::InvalidClient);
        };
        if confidential
            && (client_secret.is_empty()
                || !constant_time_string_equal(&stored_secret, client_secret))
        {
            return Err(OAuthTokenRevocationError::InvalidClient);
        }
        let Some(token) = token.filter(|token| !token.is_empty()) else {
            transaction.commit().await?;
            return Ok(());
        };
        let token_row = if token_type_hint == Some("refresh_token") {
            sqlx::query_as::<_, (i64, Option<i64>, Option<i64>)>(
                "SELECT access_token.id, access_token.application_id, token_user.account_id \
                   FROM oauth_access_tokens access_token \
                   LEFT JOIN users token_user ON token_user.id = access_token.resource_owner_id \
                  WHERE access_token.refresh_token = $1 \
                  LIMIT 1 FOR UPDATE OF access_token",
            )
            .bind(token)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            sqlx::query_as::<_, (i64, Option<i64>, Option<i64>)>(
                "SELECT access_token.id, access_token.application_id, token_user.account_id \
                   FROM oauth_access_tokens access_token \
                   LEFT JOIN users token_user ON token_user.id = access_token.resource_owner_id \
                  WHERE access_token.token = $1 \
                 LIMIT 1 FOR UPDATE OF access_token",
            )
            .bind(token)
            .fetch_optional(&mut *transaction)
            .await?
            .or(sqlx::query_as::<_, (i64, Option<i64>, Option<i64>)>(
                "SELECT access_token.id, access_token.application_id, token_user.account_id \
                   FROM oauth_access_tokens access_token \
                   LEFT JOIN users token_user ON token_user.id = access_token.resource_owner_id \
                  WHERE access_token.refresh_token = $1 \
                     LIMIT 1 FOR UPDATE OF access_token",
            )
            .bind(token)
            .fetch_optional(&mut *transaction)
            .await?)
        };
        let Some((token_id, token_application_id, account_id)) = token_row else {
            transaction.commit().await?;
            return Ok(());
        };
        if token_application_id != Some(application_id) {
            return Err(OAuthTokenRevocationError::UnauthorizedClient);
        }
        sqlx::query(
            "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, clock_timestamp()) \
             WHERE id = $1",
        )
        .bind(token_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM web_push_subscriptions WHERE access_token_id = $1")
            .bind(token_id)
            .execute(&mut *transaction)
            .await?;
        if let Some(account_id) = account_id {
            record_token_kill_stream_event(&mut transaction, account_id, token_id).await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}
