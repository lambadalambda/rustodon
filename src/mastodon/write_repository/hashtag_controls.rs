//! Account-serialized local hashtag relationships. No peer projection is emitted.
use super::{
    AuthenticatedBearer, WRITE_ACCOUNTS, WRITE_FOLLOWS, WriteError, WriteRepository,
    ensure_account_write_allowed_in, lock_account_scope, normalize_hashtag, write_account,
};
use crate::mastodon::hashtag::{display_name, featured_name, valid_name};

impl WriteRepository {
    /// Mutates a local tag relationship and returns its persisted tag identity.
    /// `collection_name` preserves `FeaturedTag`'s name-based duplicate semantics.
    pub(crate) async fn set_tag_relationship(
        &self,
        authenticated: &AuthenticatedBearer,
        name: &str,
        featured: bool,
        enabled: bool,
        collection_name: bool,
    ) -> Result<Option<i64>, WriteError> {
        let scopes = if featured {
            WRITE_ACCOUNTS
        } else {
            WRITE_FOLLOWS
        };
        let account_id = write_account(authenticated, scopes)?;
        let name = if collection_name {
            featured_name(name)
        } else {
            name
        };
        if !valid_name(name) {
            return Err(if collection_name {
                WriteError::Validation("Name is invalid")
            } else {
                WriteError::NotFound
            });
        }
        let normalized = normalize_hashtag(name);
        if enabled && !valid_name(&normalized) {
            return Err(WriteError::Validation("Name is invalid"));
        }
        let mut tx = self.pool.begin().await?;
        lock_account_scope(&mut tx, account_id).await?;
        ensure_account_write_allowed_in(&mut tx, account_id).await?;
        let mut tag_id: Option<i64> =
            sqlx::query_scalar("SELECT id FROM tags WHERE lower(name) = lower($1)")
                .bind(&normalized)
                .fetch_optional(&mut *tx)
                .await?;
        if enabled && tag_id.is_none() {
            sqlx::query("INSERT INTO tags (name, display_name, created_at, updated_at) VALUES ($1, $2, now(), now()) ON CONFLICT DO NOTHING")
                .bind(&normalized).bind(display_name(name)).execute(&mut *tx).await?;
            tag_id = sqlx::query_scalar("SELECT id FROM tags WHERE lower(name) = lower($1)")
                .bind(&normalized)
                .fetch_optional(&mut *tx)
                .await?;
        }
        if let Some(tag_id) = tag_id {
            if featured {
                if enabled {
                    let existing: Option<(i64, Option<String>)> = sqlx::query_as(
                        "SELECT id, name FROM featured_tags WHERE account_id = $1 AND tag_id = $2",
                    )
                    .bind(account_id)
                    .bind(tag_id)
                    .fetch_optional(&mut *tx)
                    .await?;
                    if let Some((_, existing_name)) = existing {
                        if collection_name && existing_name.as_deref() != Some(name) {
                            return Err(WriteError::Validation("Tag has already been taken"));
                        }
                    } else {
                        let count: i64 = sqlx::query_scalar(
                            "SELECT count(*) FROM featured_tags WHERE account_id = $1",
                        )
                        .bind(account_id)
                        .fetch_one(&mut *tx)
                        .await?;
                        if count >= 10 {
                            return Err(WriteError::Validation(
                                "You have already featured the maximum number of hashtags",
                            ));
                        }
                        sqlx::query("INSERT INTO featured_tags (account_id, tag_id, name, statuses_count, last_status_at, created_at, updated_at) \
                            SELECT $1, $2, $3, count(*), max(status.created_at), now(), now() \
                            FROM statuses status JOIN statuses_tags st ON st.status_id = status.id \
                            WHERE status.account_id = $1 AND st.tag_id = $2 AND status.deleted_at IS NULL AND status.visibility IN (0, 1)")
                            .bind(account_id).bind(tag_id).bind(if collection_name {Some(name)} else {None}).execute(&mut *tx).await?;
                    }
                } else {
                    sqlx::query("DELETE FROM featured_tags WHERE account_id = $1 AND tag_id = $2")
                        .bind(account_id)
                        .bind(tag_id)
                        .execute(&mut *tx)
                        .await?;
                }
            } else if enabled {
                sqlx::query("INSERT INTO tag_follows (account_id, tag_id, created_at, updated_at) VALUES ($1, $2, now(), now()) ON CONFLICT (account_id, tag_id) DO NOTHING")
                    .bind(account_id).bind(tag_id).execute(&mut *tx).await?;
            } else {
                sqlx::query("DELETE FROM tag_follows WHERE account_id = $1 AND tag_id = $2")
                    .bind(account_id)
                    .bind(tag_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(tag_id)
    }

    pub(crate) async fn remove_featured_tag(
        &self,
        authenticated: &AuthenticatedBearer,
        id: i64,
    ) -> Result<(), WriteError> {
        let account_id = write_account(authenticated, WRITE_ACCOUNTS)?;
        let mut tx = self.pool.begin().await?;
        lock_account_scope(&mut tx, account_id).await?;
        ensure_account_write_allowed_in(&mut tx, account_id).await?;
        let deleted = sqlx::query("DELETE FROM featured_tags WHERE account_id = $1 AND id = $2")
            .bind(account_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if deleted.rows_affected() == 0 {
            return Err(WriteError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
}
