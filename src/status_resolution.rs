//! Bounded exact-URI status materialization, without inbox delivery authority.
//!
//! This is deliberately not URL discovery: HTML, collections and arbitrary
//! alternate object identities are unsupported.
use serde_json::Value;
use url::Url;

use crate::mastodon::{HttpSignatureSigner, Repository, WriteError, WriteRepository, activitypub};
use crate::remote::{
    RemoteAccountResolver, RemoteFetcher, canonical_remote_domain_from_url, validate_remote_url,
};

pub(crate) struct RemoteStatusResolver<'a> {
    pub repository: &'a Repository,
    pub writer: &'a WriteRepository,
    pub fetcher: &'a RemoteFetcher,
    pub origin: &'a Url,
    pub limited_federation: bool,
}

impl RemoteStatusResolver<'_> {
    async fn domain_allowed(&self, viewer: i64, url: &Url) -> Result<bool, ()> {
        let domain = canonical_remote_domain_from_url(url).map_err(|_| ())?;
        if !self
            .repository
            .remote_domain_allowed(&domain, self.limited_federation)
            .await
            .map_err(|_| ())?
        {
            return Ok(false);
        }
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM account_domain_blocks WHERE account_id=$1 AND domain=$2)",
        )
        .bind(viewer)
        .bind(domain)
        .fetch_one(self.writer.pool())
        .await
        .map_err(|_| ())?;
        Ok(!blocked)
    }

    /// Network/representation denials are an empty search, database failures are errors.
    #[allow(clippy::too_many_lines)]
    pub async fn resolve(&self, viewer: i64, requested: &str) -> Result<Option<String>, ()> {
        let Ok(url) = Url::parse(requested) else {
            return Ok(None);
        };
        if validate_remote_url(&url).is_err()
            || url.host_str() == self.origin.host_str()
            || !self.domain_allowed(viewer, &url).await?
        {
            return Ok(None);
        }
        let tombstoned: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tombstones WHERE uri=$1)")
                .bind(requested)
                .fetch_one(self.writer.pool())
                .await
                .map_err(|_| ())?;
        if tombstoned {
            return Ok(None);
        }
        let instance = self
            .repository
            .account(-99)
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        let Some(key) = instance.private_key.as_ref().filter(|key| key.is_present()) else {
            return Ok(None);
        };
        let key_id = format!(
            "{}#main-key",
            activitypub::actor_url(self.origin, &instance)
        );
        let signer = HttpSignatureSigner {
            key_id: &key_id,
            private_key_pem: key.as_str(),
        };
        let Ok(response) = self
            .fetcher
            .get_signed(
                url.clone(),
                &["application/activity+json", "application/ld+json"],
                &signer,
            )
            .await
        else {
            return Ok(None);
        };
        // Even a valid transport redirect cannot grant another origin authority.
        if response.url.origin() != url.origin() {
            return Ok(None);
        }
        let Ok(document) = serde_json::from_slice::<Value>(&response.body) else {
            return Ok(None);
        };
        let Some(identity) = document.get("id").and_then(Value::as_str) else {
            return Ok(None);
        };
        // The existing signed transport can follow a same-origin redirect to a
        // canonical object. It cannot establish an arbitrary advertised identity.
        if identity != requested && identity != response.url.as_str() {
            return Ok(None);
        }
        if identity != requested {
            match self
                .repository
                .known_search_status_id(identity, self.origin.as_str(), viewer)
                .await
                .map_err(|_| ())?
            {
                crate::mastodon::KnownSearchStatus::Denied => return Ok(None),
                crate::mastodon::KnownSearchStatus::Found(_) => {
                    return Ok(Some(identity.to_owned()));
                }
                crate::mastodon::KnownSearchStatus::Unknown => (),
            }
        }
        let Some(actor_uri) = document.get("attributedTo").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Ok(actor_location) = Url::parse(actor_uri) else {
            return Ok(None);
        };
        if validate_remote_url(&actor_location).is_err()
            || actor_location.origin() != url.origin()
            || !self.domain_allowed(viewer, &actor_location).await?
        {
            return Ok(None);
        }
        // Parse and authorize before resolving/persisting an unknown actor. This
        // shares the ingestion parser, not an approximation of its audience.
        match self
            .writer
            .remote_note_search_allowed(viewer, actor_uri, &document, self.origin.as_str())
            .await
        {
            Ok(true) => (),
            Ok(false) | Err(WriteError::InvalidInput(_)) => return Ok(None),
            Err(_) => return Err(()),
        }
        let known: Option<i64> =
            sqlx::query_scalar("SELECT id FROM accounts WHERE uri=$1 AND domain IS NOT NULL")
                .bind(actor_uri)
                .fetch_optional(self.writer.pool())
                .await
                .map_err(|_| ())?;
        let actor_id = if let Some(id) = known {
            id
        } else {
            let Ok(actor) = RemoteAccountResolver::new(self.fetcher.clone())
                .resolve_actor_uri_with_signer(&actor_location, Some(&signer))
                .await
            else {
                return Ok(None);
            };
            if actor.id != actor_location || actor.suspended {
                return Ok(None);
            }
            self.writer
                .upsert_remote_actor(
                    &actor.username,
                    &actor.domain,
                    self.limited_federation,
                    &actor,
                )
                .await
                .map_err(|_| ())?
        };
        // NEVER pass the searching viewer here, or ensure a delivery mention.
        self.writer
            .apply_remote_note_create(actor_id, actor_uri, &document, None, self.origin.as_str())
            .await
            .map_err(|_| ())?;
        Ok(Some(identity.to_owned()))
    }
}
