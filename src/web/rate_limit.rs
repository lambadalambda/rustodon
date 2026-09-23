//! In-process and shared `PostgreSQL` rate limiters and signature fetch circuits.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

pub(super) const MAX_RATE_LIMIT_WINDOWS: usize = 65_536;

#[derive(Clone, Copy, Debug)]
pub(super) struct RateLimitExceeded {
    pub(super) limit: usize,
    pub(super) period: StdDuration,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RateLimitStatus {
    pub(super) limit: usize,
    pub(super) remaining: usize,
    pub(super) period: StdDuration,
}

pub(super) struct RateLimitWindow {
    pub(super) bucket: u64,
    pub(super) attempts: usize,
    pub(super) expires_at: u64,
}

#[derive(Default)]
pub(super) struct AttemptLimiterState {
    pub(super) windows: HashMap<String, RateLimitWindow>,
    pub(super) expirations: BinaryHeap<Reverse<(u64, String, u64)>>,
}

#[derive(Clone, Default)]
pub(super) struct AttemptLimiter {
    pub(super) state: Arc<Mutex<AttemptLimiterState>>,
}

impl AttemptLimiter {
    pub(super) fn try_allow<I>(&self, keys: I) -> Result<(), RateLimitExceeded>
    where
        I: IntoIterator<Item = (String, usize, StdDuration)>,
    {
        self.try_allow_at(keys, unix_timestamp_seconds())
    }

    #[cfg(test)]
    pub(super) fn allow_at<I>(&self, keys: I, now: u64) -> bool
    where
        I: IntoIterator<Item = (String, usize, StdDuration)>,
    {
        self.try_allow_at(keys, now).is_ok()
    }

    pub(super) fn try_allow_at<I>(&self, keys: I, now: u64) -> Result<(), RateLimitExceeded>
    where
        I: IntoIterator<Item = (String, usize, StdDuration)>,
    {
        let keys = keys.into_iter().collect::<Vec<_>>();
        if keys.is_empty()
            || keys
                .iter()
                .any(|(_, limit, period)| *limit == 0 || period.as_secs() == 0)
        {
            return Err(RateLimitExceeded {
                limit: 0,
                period: StdDuration::from_secs(1),
            });
        }
        let Ok(mut state) = self.state.lock() else {
            return Err(RateLimitExceeded {
                limit: keys[0].1,
                period: keys[0].2,
            });
        };
        state.purge_expired(now);
        for (key, _, period) in &keys {
            let bucket = rate_limit_bucket(now, *period);
            if state
                .windows
                .get(key)
                .is_some_and(|window| window.bucket != bucket)
            {
                state.windows.remove(key);
            }
        }
        let new_keys = keys
            .iter()
            .filter(|(key, _, _)| !state.windows.contains_key(key))
            .count();
        if state.windows.len().saturating_add(new_keys) > MAX_RATE_LIMIT_WINDOWS {
            return Err(RateLimitExceeded {
                limit: keys[0].1,
                period: keys[0].2,
            });
        }
        if let Some((_, limit, period)) = keys.iter().find(|(key, limit, _)| {
            state
                .windows
                .get(key)
                .is_some_and(|window| window.attempts >= *limit)
        }) {
            return Err(RateLimitExceeded {
                limit: *limit,
                period: *period,
            });
        }
        for (key, _, period) in keys {
            let bucket = rate_limit_bucket(now, period);
            let expires_at = bucket.saturating_add(1).saturating_mul(period.as_secs());
            if let Some(window) = state.windows.get_mut(&key) {
                window.attempts += 1;
            } else {
                state.windows.insert(
                    key.clone(),
                    RateLimitWindow {
                        bucket,
                        attempts: 1,
                        expires_at,
                    },
                );
                state.expirations.push(Reverse((expires_at, key, bucket)));
            }
        }
        Ok(())
    }
}

impl AttemptLimiterState {
    pub(super) fn purge_expired(&mut self, now: u64) {
        while let Some(Reverse((expires_at, key, bucket))) = self.expirations.peek().cloned() {
            if expires_at > now {
                break;
            }
            self.expirations.pop();
            if self
                .windows
                .get(&key)
                .is_some_and(|window| window.bucket == bucket && window.expires_at <= now)
            {
                self.windows.remove(&key);
            }
        }
    }
}

pub(super) fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(super) fn rate_limit_bucket(now: u64, period: StdDuration) -> u64 {
    now / period.as_secs()
}

#[derive(Clone)]
pub(super) struct SharedRateLimiter {
    pub(super) pool: PgPool,
    #[cfg(feature = "test-support")]
    pub(super) fixed_time: Option<i64>,
}

pub(super) struct SharedRateLimitKey {
    pub(super) window_key: String,
    pub(super) limit: usize,
    pub(super) period: StdDuration,
    pub(super) bucket: i64,
    pub(super) expires_at: i64,
}

impl SharedRateLimiter {
    pub(super) const fn new(pool: PgPool) -> Self {
        Self {
            pool,
            #[cfg(feature = "test-support")]
            fixed_time: None,
        }
    }

    #[cfg(feature = "test-support")]
    pub(super) fn with_fixed_time(mut self, unix_seconds: i64) -> Self {
        self.fixed_time = Some(unix_seconds);
        self
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn try_allow<I>(&self, keys: I) -> Result<(), RateLimitExceeded>
    where
        I: IntoIterator<Item = (String, usize, StdDuration)>,
    {
        let keys = keys.into_iter().collect::<Vec<_>>();
        let Some((_, limit, period)) = keys.first() else {
            return Err(RateLimitExceeded {
                limit: 0,
                period: StdDuration::from_secs(1),
            });
        };
        let fallback = RateLimitExceeded {
            limit: *limit,
            period: *period,
        };
        if keys
            .iter()
            .any(|(_, limit, period)| *limit == 0 || period.as_secs() == 0)
        {
            return Err(fallback);
        }
        let now = unix_timestamp_seconds();
        #[cfg(feature = "test-support")]
        let now = match self.fixed_time {
            Some(now) => u64::try_from(now).map_err(|_| fallback)?,
            None => now,
        };
        let mut shared_keys = Vec::with_capacity(keys.len());
        for (window_key, limit, period) in keys {
            let Ok(period_seconds) = i64::try_from(period.as_secs()) else {
                return Err(fallback);
            };
            let Ok(bucket) = i64::try_from(now / period.as_secs()) else {
                return Err(fallback);
            };
            let Some(expires_at) = bucket
                .checked_add(1)
                .and_then(|bucket| bucket.checked_mul(period_seconds))
            else {
                return Err(fallback);
            };
            if i32::try_from(limit).is_err() {
                return Err(fallback);
            }
            shared_keys.push(SharedRateLimitKey {
                window_key,
                limit,
                period,
                bucket,
                expires_at,
            });
        }

        let mut lock_keys = shared_keys
            .iter()
            .map(|key| key.window_key.as_str())
            .collect::<Vec<_>>();
        lock_keys.sort_unstable();
        let Ok(mut transaction) = self.pool.begin().await else {
            return Err(fallback);
        };
        for window_key in lock_keys {
            if sqlx::query(
                "SELECT pg_catalog.pg_advisory_xact_lock(\
                   pg_catalog.hashtext('rustodon:rate_limit:' || $1))",
            )
            .bind(window_key)
            .execute(&mut *transaction)
            .await
            .is_err()
            {
                return Err(fallback);
            }
        }
        for key in &shared_keys {
            let delete_expired = sqlx::query(
                "DELETE FROM rustodon.rate_limit_windows \
                 WHERE window_key = $1 AND expires_at <= clock_timestamp()",
            )
            .bind(&key.window_key);
            #[cfg(feature = "test-support")]
            let delete_expired = match self.fixed_time {
                Some(now) => sqlx::query(
                    "DELETE FROM rustodon.rate_limit_windows \
                     WHERE window_key = $1 AND expires_at <= to_timestamp($2::double precision)",
                )
                .bind(&key.window_key)
                .bind(now),
                None => delete_expired,
            };
            if delete_expired.execute(&mut *transaction).await.is_err() {
                return Err(fallback);
            }
        }
        for key in &shared_keys {
            let attempts = match sqlx::query_scalar::<_, i32>(
                "SELECT attempts FROM rustodon.rate_limit_windows \
                 WHERE window_key = $1 AND bucket = $2",
            )
            .bind(&key.window_key)
            .bind(key.bucket)
            .fetch_optional(&mut *transaction)
            .await
            {
                Ok(attempts) => attempts.unwrap_or_default(),
                Err(_) => return Err(fallback),
            };
            if usize::try_from(attempts).unwrap_or(usize::MAX) >= key.limit {
                return Err(RateLimitExceeded {
                    limit: key.limit,
                    period: key.period,
                });
            }
        }
        for key in &shared_keys {
            if sqlx::query(
                "INSERT INTO rustodon.rate_limit_windows \
                   (window_key, bucket, attempts, expires_at) \
                 VALUES ($1, $2, 1, to_timestamp($3::double precision)) \
                 ON CONFLICT (window_key, bucket) DO UPDATE \
                 SET attempts = rustodon.rate_limit_windows.attempts + 1",
            )
            .bind(&key.window_key)
            .bind(key.bucket)
            .bind(key.expires_at)
            .execute(&mut *transaction)
            .await
            .is_err()
            {
                return Err(fallback);
            }
        }
        if transaction.commit().await.is_err() {
            return Err(fallback);
        }
        Ok(())
    }

    pub(super) async fn circuit_open(&self, window_key: &str) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(\
               pg_catalog.hashtext('rustodon:rate_limit:' || $1))",
        )
        .bind(window_key)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "DELETE FROM rustodon.rate_limit_windows \
             WHERE window_key = $1 AND bucket = 0 AND expires_at <= clock_timestamp()",
        )
        .bind(window_key)
        .execute(&mut *transaction)
        .await?;
        let open = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(\
               SELECT 1 FROM rustodon.rate_limit_windows \
               WHERE window_key = $1 AND bucket = 0 AND expires_at > clock_timestamp())",
        )
        .bind(window_key)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(open)
    }

    pub(super) async fn record_circuit_failure(
        &self,
        window_key: &str,
        cool_off: StdDuration,
    ) -> Result<(), sqlx::Error> {
        let cool_off_seconds = i64::try_from(cool_off.as_secs())
            .map_err(|_| sqlx::Error::Protocol("signature circuit period is too large".into()))?;
        if cool_off_seconds <= 0 {
            return Err(sqlx::Error::Protocol(
                "signature circuit period must be positive".into(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(\
               pg_catalog.hashtext('rustodon:rate_limit:' || $1))",
        )
        .bind(window_key)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "DELETE FROM rustodon.rate_limit_windows \
             WHERE window_key = $1 AND bucket = 0 AND expires_at <= clock_timestamp()",
        )
        .bind(window_key)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO rustodon.rate_limit_windows \
               (window_key, bucket, attempts, expires_at) \
             VALUES ($1, 0, 1, clock_timestamp() + ($2::double precision * INTERVAL '1 second')) \
             ON CONFLICT (window_key, bucket) DO NOTHING",
        )
        .bind(window_key)
        .bind(cool_off_seconds)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await
    }
}

pub(super) async fn try_rate_limit<I>(
    local: &AttemptLimiter,
    shared: Option<&SharedRateLimiter>,
    keys: I,
) -> Result<(), RateLimitExceeded>
where
    I: IntoIterator<Item = (String, usize, StdDuration)>,
{
    let keys = keys.into_iter().collect::<Vec<_>>();
    match shared {
        Some(shared) => shared.try_allow(keys).await,
        None => local.try_allow(keys),
    }
}

#[derive(Clone, Default)]
pub(super) struct PasswordResetLimiter {
    pub(super) limiter: AttemptLimiter,
}

impl PasswordResetLimiter {
    pub(super) fn keys(client_ip: IpAddr, email: &str) -> [(String, usize, StdDuration); 2] {
        let normalized_email = email.trim().to_ascii_lowercase();
        let email_digest = format!("{:x}", Sha256::digest(normalized_email.as_bytes()));
        [
            (
                format!("password_reset:ip:{}", attempt_ip_bucket(client_ip)),
                25,
                StdDuration::from_mins(5),
            ),
            (
                format!("password_reset:email:{email_digest}"),
                5,
                StdDuration::from_mins(30),
            ),
        ]
    }

    #[cfg(test)]
    pub(super) fn check(&self, client_ip: IpAddr, email: &str) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip, email))
    }

    pub(super) async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
        email: &str,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip, email)).await
    }
}

#[derive(Clone, Default)]
pub(super) struct BrowserLoginLimiter {
    pub(super) limiter: AttemptLimiter,
}

impl BrowserLoginLimiter {
    pub(super) fn keys(client_ip: IpAddr, email: &str) -> [(String, usize, StdDuration); 2] {
        let normalized_email = email.trim().to_ascii_lowercase();
        let email_digest = format!("{:x}", Sha256::digest(normalized_email.as_bytes()));
        [
            (
                format!("browser_login:ip:{}", attempt_ip_bucket(client_ip)),
                25,
                StdDuration::from_mins(5),
            ),
            (
                format!("browser_login:email:{email_digest}"),
                25,
                StdDuration::from_hours(1),
            ),
        ]
    }

    #[cfg(test)]
    pub(super) fn check(&self, client_ip: IpAddr, email: &str) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip, email))
    }

    pub(super) async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
        email: &str,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip, email)).await
    }
}

#[derive(Clone, Default)]
pub(super) struct BrowserReauthenticationLimiter {
    pub(super) limiter: AttemptLimiter,
    #[cfg(feature = "test-support")]
    pub(super) fixed_time: Option<i64>,
}

impl BrowserReauthenticationLimiter {
    pub(super) fn keys(client_ip: IpAddr, user_id: i64) -> [(String, usize, StdDuration); 2] {
        [
            (
                format!(
                    "browser_reauthentication:ip:{}",
                    attempt_ip_bucket(client_ip)
                ),
                25,
                StdDuration::from_mins(5),
            ),
            (
                format!("browser_reauthentication:user:{user_id}"),
                10,
                StdDuration::from_hours(1),
            ),
        ]
    }

    pub(super) async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
        user_id: i64,
    ) -> Result<(), RateLimitExceeded> {
        #[cfg(feature = "test-support")]
        if let (Some(now), Some(shared)) = (self.fixed_time, shared) {
            // Freeze only this reauthentication call, not the state's shared
            // limiter used by login, password reset, media, and other routes.
            return shared
                .clone()
                .with_fixed_time(now)
                .try_allow(Self::keys(client_ip, user_id))
                .await;
        }
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip, user_id)).await
    }
}

#[derive(Clone, Default)]
pub(super) struct OAuthApplicationLimiter {
    pub(super) limiter: AttemptLimiter,
}

impl OAuthApplicationLimiter {
    pub(super) fn keys(client_ip: IpAddr) -> [(String, usize, StdDuration); 1] {
        [(
            format!("oauth_application:ip:{}", attempt_ip_bucket(client_ip)),
            5,
            StdDuration::from_mins(10),
        )]
    }

    #[cfg(test)]
    pub(super) fn check(&self, client_ip: IpAddr) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip))
    }

    pub(super) async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip)).await
    }
}

#[derive(Clone, Default)]
pub(super) struct MediaProxyLimiter {
    pub(super) limiter: AttemptLimiter,
}

impl MediaProxyLimiter {
    pub(super) fn keys(client_ip: IpAddr) -> [(String, usize, StdDuration); 1] {
        [(
            format!("media_proxy:ip:{}", attempt_ip_bucket(client_ip)),
            30,
            StdDuration::from_mins(10),
        )]
    }

    #[cfg(test)]
    pub(super) fn check(&self, client_ip: IpAddr) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip))
    }

    pub(super) async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip)).await
    }
}

#[derive(Clone, Default)]
pub(super) struct MediaUploadLimiter {
    pub(super) limiter: AttemptLimiter,
}

impl MediaUploadLimiter {
    pub(super) fn keys(user_id: i64) -> [(String, usize, StdDuration); 1] {
        [(
            format!("media_upload:user:{user_id}"),
            MEDIA_UPLOAD_RATE_LIMIT,
            MEDIA_UPLOAD_RATE_LIMIT_PERIOD,
        )]
    }

    #[cfg(test)]
    pub(super) fn check(&self, user_id: i64) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(user_id))
    }

    pub(super) async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        user_id: i64,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(user_id)).await
    }
}

#[derive(Clone, Default)]
pub(super) struct ActivityPubInboxLimiter {
    pub(super) limiter: AttemptLimiter,
}

impl ActivityPubInboxLimiter {
    pub(super) fn keys(client_ip: IpAddr) -> [(String, usize, StdDuration); 1] {
        [(
            format!("activitypub_inbox:ip:{}", attempt_ip_bucket(client_ip)),
            ACTIVITYPUB_INBOX_RATE_LIMIT,
            ACTIVITYPUB_INBOX_RATE_LIMIT_PERIOD,
        )]
    }

    #[cfg(test)]
    pub(super) fn check(&self, client_ip: IpAddr) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip))
    }

    pub(super) async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip)).await
    }
}

#[derive(Clone, Default)]
pub(super) struct RemoteAccountResolutionLimiter {
    pub(super) limiter: AttemptLimiter,
}

impl RemoteAccountResolutionLimiter {
    pub(super) fn keys(
        client_ip: IpAddr,
        username: &str,
        domain: &str,
    ) -> [(String, usize, StdDuration); 2] {
        let handle = format!("{}@{}", username.trim(), domain.trim()).to_ascii_lowercase();
        let handle_digest = format!("{:x}", Sha256::digest(handle.as_bytes()));
        [
            (
                format!(
                    "remote_account_resolution:ip:{}",
                    attempt_ip_bucket(client_ip)
                ),
                10,
                StdDuration::from_mins(1),
            ),
            (
                format!("remote_account_resolution:handle:{handle_digest}"),
                1,
                StdDuration::from_mins(5),
            ),
        ]
    }

    #[cfg(test)]
    pub(super) fn check(
        &self,
        client_ip: IpAddr,
        username: &str,
        domain: &str,
    ) -> Result<(), RateLimitExceeded> {
        self.limiter
            .try_allow(Self::keys(client_ip, username, domain))
    }

    pub(super) async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
        username: &str,
        domain: &str,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(
            &self.limiter,
            shared,
            Self::keys(client_ip, username, domain),
        )
        .await
    }
}

#[derive(Clone, Default)]
pub(super) struct SignatureFetchCircuit {
    pub(super) state: Arc<Mutex<SignatureFetchCircuitState>>,
}

#[derive(Default)]
pub(super) struct SignatureFetchCircuitState {
    pub(super) failures: HashMap<IpAddr, u64>,
    pub(super) expirations: BinaryHeap<Reverse<(u64, IpAddr)>>,
}

impl SignatureFetchCircuit {
    pub(super) fn allow(&self, client_ip: IpAddr) -> bool {
        self.allow_at(client_ip, unix_timestamp_seconds())
    }

    pub(super) fn allow_at(&self, client_ip: IpAddr, now: u64) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        state.purge_expired(now);
        !state.failures.contains_key(&client_ip)
            && state.failures.len() < MAX_SIGNATURE_FETCH_CIRCUITS
    }

    pub(super) fn record_failure(&self, client_ip: IpAddr) {
        self.record_failure_at(client_ip, unix_timestamp_seconds());
    }

    pub(super) fn record_failure_at(&self, client_ip: IpAddr, now: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.purge_expired(now);
        if state.failures.len() >= MAX_SIGNATURE_FETCH_CIRCUITS
            || state.failures.contains_key(&client_ip)
        {
            return;
        }
        let expires_at = now.saturating_add(SIGNATURE_FETCH_COOL_OFF.as_secs());
        state.failures.insert(client_ip, expires_at);
        state.expirations.push(Reverse((expires_at, client_ip)));
    }
}

impl SignatureFetchCircuitState {
    pub(super) fn purge_expired(&mut self, now: u64) {
        while let Some(Reverse((expires_at, client_ip))) = self.expirations.peek().copied() {
            if expires_at > now {
                break;
            }
            self.expirations.pop();
            if self
                .failures
                .get(&client_ip)
                .is_some_and(|expiration| *expiration == expires_at)
            {
                self.failures.remove(&client_ip);
            }
        }
    }
}

pub(super) fn attempt_ip_bucket(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => {
            let network = u128::from(ip) & (!0_u128 << 64);
            format!("{}/64", Ipv6Addr::from(network))
        }
    }
}

pub(super) fn signature_fetch_circuit_key(client_ip: IpAddr) -> String {
    format!(
        "signature_fetch_circuit:ip:{}",
        attempt_ip_bucket(client_ip)
    )
}
