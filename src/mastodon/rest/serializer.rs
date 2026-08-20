use std::fmt;

use chrono::NaiveDateTime;
use url::Url;

use crate::paperclip::{PaperclipAttachment, PaperclipMetadata, encode_url_path, rails_blank};

use super::{
    AccountProjection, AccountRelationshipProjection, AccountWarningProjection, ApiDate,
    ApiDateTime, ApiSecondDateTime, CollectionProjection, CredentialAccountProjection,
    CustomEmojiProjection, DecimalId, FeaturedTagProjection, FilterProjection,
    FilterResultProjection, GroupedNotificationsProjection, HtmlFormatter, InstanceProjection,
    ListProjection, MarkerProjection, MediaAttachmentProjection, MentionProjection,
    NotificationGroupProjection, NotificationProjection, PollProjection, PreviewCardProjection,
    QuoteProjection, QuoteTargetAccess, QuoteTargetLinkProjection, ReportProjection, RestAccount,
    RestAccountField, RestAccountRole, RestAccountWarning, RestAnnualReport, RestAppeal,
    RestApplication, RestCollection, RestCollectionItem, RestCollectionWithAccounts,
    RestCredentialAccount, RestCredentialSource, RestCustomEmoji, RestFallback,
    RestFeatureApproval, RestFeaturedTag, RestFilter, RestFilterKeyword, RestFilterResult,
    RestFilterStatus, RestGroupedNotifications, RestInstanceV1, RestInstanceV2, RestList,
    RestMarker, RestMediaAttachment, RestMention, RestMutedAccount, RestNotification,
    RestNotificationGroup, RestPartialAccount, RestPoll, RestPollOption, RestPreviewCard,
    RestPreviewCardAuthor, RestQuote, RestQuoteApproval, RestQuotePayload, RestRelationship,
    RestReport, RestRole, RestRule, RestSeveranceEvent, RestShallowQuote, RestShallowTag,
    RestStatus, RestStatusContext, RestStatusEdit, RestStatusEditPoll, RestStatusEditPollOption,
    RestStatusSource, RestTag, RestTagHistory, SeveranceEventProjection, StatusContextProjection,
    StatusEditProjection, StatusProjection, TagProjection,
};
use crate::mastodon::AccountIdScheme;

const DEFAULT_AVATAR: &str = "avatars/original/missing.png";
const DEFAULT_HEADER: &str = "headers/original/missing.png";
const SUPPORTED_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/heic",
    "image/heif",
    "image/webp",
    "image/avif",
    "video/webm",
    "video/mp4",
    "video/quicktime",
    "video/ogg",
    "audio/wave",
    "audio/wav",
    "audio/x-wav",
    "audio/x-pn-wave",
    "audio/vnd.wave",
    "audio/ogg",
    "audio/vorbis",
    "audio/mpeg",
    "audio/mp3",
    "audio/webm",
    "audio/flac",
    "audio/aac",
    "audio/m4a",
    "audio/x-m4a",
    "audio/mp4",
    "audio/3gpp",
    "video/x-ms-asf",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusShape {
    Full,
    Shallow,
    Source,
}

#[derive(Clone, Debug)]
pub struct RestSerializer<'a> {
    origin: &'a Url,
    local_domain: &'a str,
    media_root_url: &'a str,
    now: NaiveDateTime,
}

#[allow(clippy::missing_errors_doc, clippy::too_many_lines)]
impl<'a> RestSerializer<'a> {
    #[must_use]
    pub const fn new(
        origin: &'a Url,
        local_domain: &'a str,
        media_root_url: &'a str,
        now: NaiveDateTime,
    ) -> Self {
        Self {
            origin,
            local_domain,
            media_root_url,
            now,
        }
    }

    pub fn account(&self, account: &AccountProjection) -> Result<RestAccount, RestError> {
        self.account_with_depth(account, true)
    }

    #[must_use]
    pub fn status_source(&self, status: &StatusProjection) -> RestStatusSource {
        RestStatusSource {
            id: DecimalId::new(status.id),
            text: status.text.clone(),
            spoiler_text: status.spoiler_text.clone(),
        }
    }

    pub fn status_edit(&self, edit: &StatusEditProjection) -> Result<RestStatusEdit, RestError> {
        let formatter = HtmlFormatter::new(self.origin, self.local_domain);
        let content = if edit.account.local() {
            formatter.local_text(&edit.text, &[], None).into_string()
        } else {
            formatter.remote_fragment(&edit.text).into_string()
        };
        Ok(RestStatusEdit {
            account: self.account(&edit.account)?,
            content,
            spoiler_text: edit.spoiler_text.clone(),
            sensitive: edit.sensitive,
            created_at: ApiDateTime::new(edit.created_at),
            media_attachments: edit
                .media_attachments
                .iter()
                .map(|media| self.media_attachment(media))
                .collect(),
            emojis: edit
                .emojis
                .iter()
                .map(|emoji| self.custom_emoji(emoji))
                .collect(),
            quote: edit
                .quote
                .as_ref()
                .map(|quote| self.quote(quote, StatusShape::Full))
                .transpose()?,
            poll: edit
                .poll_options
                .as_ref()
                .map(|options| RestStatusEditPoll {
                    options: options
                        .iter()
                        .map(|title| RestStatusEditPollOption {
                            title: title.clone(),
                        })
                        .collect(),
                }),
        })
    }

    pub fn muted_account(
        &self,
        account: &AccountProjection,
        mute_expires_at: Option<NaiveDateTime>,
    ) -> Result<RestMutedAccount, RestError> {
        Ok(RestMutedAccount {
            account: self.account(account)?,
            mute_expires_at: mute_expires_at
                .filter(|expires_at| *expires_at >= self.now)
                .map(ApiSecondDateTime::new),
        })
    }

    pub fn credential_account(
        &self,
        credential: &CredentialAccountProjection,
    ) -> Result<RestCredentialAccount, RestError> {
        let account = self.account(&credential.account)?;
        Ok(RestCredentialAccount {
            source: RestCredentialSource {
                privacy: credential.privacy.clone(),
                sensitive: credential.sensitive,
                language: credential.language.clone(),
                note: credential.account.note.clone(),
                fields: credential
                    .account
                    .fields
                    .iter()
                    .map(|field| RestAccountField {
                        name: field.name.clone(),
                        value: field.value.clone(),
                        verified_at: field.verified_at.map(ApiDateTime::new),
                    })
                    .collect(),
                follow_requests_count: credential.follow_requests_count,
                hide_collections: credential.account.hide_collections,
                discoverable: credential.account.discoverable,
                indexable: credential.account.indexable,
                attribution_domains: credential.attribution_domains.clone(),
                quote_policy: credential.quote_policy.clone(),
            },
            role: RestRole {
                id: DecimalId::new(credential.role.id),
                name: credential.role.name.clone(),
                permissions: credential.role.permissions.to_string(),
                color: credential.role.color.clone(),
                highlighted: credential.role.highlighted,
                collection_limit: credential.role.collection_limit,
            },
            account,
        })
    }

    #[must_use]
    pub fn relationship(&self, relationship: &AccountRelationshipProjection) -> RestRelationship {
        RestRelationship {
            id: DecimalId::new(relationship.target_account_id),
            following: relationship.following,
            showing_reblogs: relationship.showing_reblogs,
            notifying: relationship.notifying,
            languages: relationship.languages.clone(),
            followed_by: relationship.followed_by,
            blocking: relationship.blocking,
            blocked_by: relationship.blocked_by,
            muting: relationship.muting,
            muting_notifications: relationship.muting_notifications,
            muting_expires_at: relationship.muting_expires_at.map(ApiSecondDateTime::new),
            requested: relationship.requested,
            requested_by: relationship.requested_by,
            domain_blocking: relationship.domain_blocking,
            endorsed: relationship.endorsed,
            note: relationship.note.clone(),
        }
    }

    pub fn instance_v1(&self, instance: &InstanceProjection) -> Result<RestInstanceV1, RestError> {
        let registrations =
            instance.registrations_mode != "none" && !instance.runtime.single_user_mode;
        Ok(RestInstanceV1 {
            uri: instance.runtime.domain.clone(),
            title: instance.title.clone(),
            short_description: instance.short_description.clone(),
            description: instance.legacy_description.clone(),
            email: instance.contact_email.clone(),
            version: instance.runtime.version.clone(),
            urls: serde_json::json!({
                "streaming_api": instance.runtime.streaming_api,
            }),
            stats: serde_json::json!({
                "user_count": instance.user_count,
                "status_count": instance.status_count,
                "domain_count": instance.domain_count,
            }),
            thumbnail: Some(instance.runtime.thumbnail_versions.as_ref().map_or_else(
                || instance.runtime.thumbnail_url.clone(),
                |(one_x, _)| one_x.clone(),
            )),
            languages: instance.runtime.languages.clone(),
            registrations,
            approval_required: instance.registrations_mode == "approved",
            invites_enabled: instance.invites_enabled,
            configuration: serde_json::json!({
                "accounts": {
                    "max_featured_tags": 10,
                },
                "statuses": {
                    "max_characters": 500,
                    "max_media_attachments": 4,
                    "characters_reserved_per_url": 23,
                },
                "media_attachments": {
                    "supported_mime_types": SUPPORTED_MIME_TYPES,
                    "image_size_limit": 16_777_216,
                    "image_matrix_limit": 33_177_600,
                    "video_size_limit": 103_809_024,
                    "video_frame_rate_limit": 120,
                    "video_matrix_limit": 8_294_400,
                },
                "polls": {
                    "max_options": 4,
                    "max_characters_per_option": 50,
                    "min_expiration": 300,
                    "max_expiration": 2_629_746,
                },
            }),
            contact_account: instance
                .contact_account
                .as_ref()
                .map(|account| self.account(account))
                .transpose()?,
            rules: Self::rules(instance),
        })
    }

    pub fn instance_v2(&self, instance: &InstanceProjection) -> Result<RestInstanceV2, RestError> {
        let registrations =
            instance.registrations_mode != "none" && !instance.runtime.single_user_mode;
        let active_month = if instance.runtime.limited_federation {
            0
        } else {
            instance.runtime.active_month
        };
        let registration_message = if registrations {
            None
        } else {
            instance
                .closed_registrations_message
                .as_deref()
                .map(|message| {
                    HtmlFormatter::new(self.origin, self.local_domain)
                        .local_text(message, &[], None)
                        .into_string()
                })
        };
        let contact_account = instance
            .contact_account
            .as_ref()
            .map(|account| self.account(account))
            .transpose()?;
        Ok(RestInstanceV2 {
            domain: instance.runtime.domain.clone(),
            title: instance.title.clone(),
            version: instance.runtime.version.clone(),
            source_url: instance.runtime.source_url.clone(),
            description: instance.short_description.clone(),
            usage: serde_json::json!({
                "users": { "active_month": active_month },
            }),
            thumbnail: instance.runtime.thumbnail_versions.as_ref().map_or_else(
                || {
                    serde_json::json!({
                        "url": instance.runtime.thumbnail_url,
                        "description": instance.runtime.thumbnail_description,
                    })
                },
                |(one_x, two_x)| {
                    serde_json::json!({
                        "url": one_x,
                        "blurhash": instance.runtime.thumbnail_blurhash,
                        "versions": { "@1x": one_x, "@2x": two_x },
                        "description": instance.runtime.thumbnail_description,
                    })
                },
            ),
            icon: instance
                .runtime
                .icons
                .iter()
                .map(|(src, size)| serde_json::json!({ "src": src, "size": size }))
                .collect(),
            languages: instance.runtime.languages.clone(),
            configuration: serde_json::json!({
                "urls": {
                    "streaming": instance.runtime.streaming_api,
                    "status": instance.status_page_url,
                    "about": self.absolute("about"),
                    "privacy_policy": self.absolute("privacy-policy"),
                    "terms_of_service": instance.runtime.terms_of_service_url,
                },
                "vapid": {
                    "public_key": instance.runtime.vapid_public_key,
                },
                "accounts": {
                    "max_display_name_length": 40,
                    "max_note_length": 500,
                    "max_avatar_description_length": 150,
                    "max_header_description_length": 150,
                    "max_featured_tags": 10,
                    "max_pinned_statuses": 5,
                    "max_profile_fields": 4,
                    "profile_field_name_limit": 255,
                    "profile_field_value_limit": 255,
                },
                "statuses": {
                    "max_characters": 500,
                    "max_media_attachments": 4,
                    "characters_reserved_per_url": 23,
                },
                "media_attachments": {
                    "description_limit": 10_000,
                    "image_matrix_limit": 33_177_600,
                    "image_size_limit": 16_777_216,
                    "supported_mime_types": SUPPORTED_MIME_TYPES,
                    "video_frame_rate_limit": 120,
                    "video_matrix_limit": 8_294_400,
                    "video_size_limit": 103_809_024,
                },
                "polls": {
                    "max_options": 4,
                    "max_characters_per_option": 50,
                    "min_expiration": 300,
                    "max_expiration": 2_629_746,
                },
                "translation": {
                    "enabled": instance.runtime.translation_enabled,
                },
                "timelines_access": {
                    "live_feeds": {
                        "local": instance.local_live_feed_access,
                        "remote": instance.remote_live_feed_access,
                    },
                    "hashtag_feeds": {
                        "local": instance.local_topic_feed_access,
                        "remote": instance.remote_topic_feed_access,
                    },
                    "trending_link_feeds": {
                        "local": instance.local_topic_feed_access,
                        "remote": instance.remote_topic_feed_access,
                    },
                },
                "limited_federation": instance.runtime.limited_federation,
            }),
            registrations: serde_json::json!({
                "enabled": registrations,
                "approval_required": instance.registrations_mode == "approved",
                "reason_required": instance.registrations_mode == "approved" && instance.require_invite_text,
                "message": registration_message,
                "min_age": instance.min_age,
                "url": instance.runtime.sso_signup_url,
            }),
            api_versions: serde_json::json!({ "mastodon": 11 }),
            wrapstodon: instance.runtime.wrapstodon,
            contact: serde_json::json!({
                "email": instance.contact_email,
                "account": contact_account,
            }),
            rules: Self::rules(instance),
        })
    }

    #[must_use]
    pub fn rules(instance: &InstanceProjection) -> Vec<RestRule> {
        instance
            .rules
            .iter()
            .map(|rule| {
                let translations = rule
                    .translations
                    .iter()
                    .map(|(language, (text, hint))| {
                        (
                            language.clone(),
                            serde_json::json!({ "text": text, "hint": hint }),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>();
                RestRule {
                    id: DecimalId::new(rule.id),
                    text: rule.text.clone(),
                    hint: rule.hint.clone(),
                    translations: serde_json::Value::Object(translations),
                }
            })
            .collect()
    }

    pub fn collection(
        &self,
        collection: &CollectionProjection,
    ) -> Result<RestCollection, RestError> {
        let formatter = HtmlFormatter::new(self.origin, self.local_domain);
        let description = if collection.local {
            collection.description.clone()
        } else {
            collection
                .description_html
                .as_deref()
                .map(|description| formatter.remote_fragment(description).into_string())
        };
        let uri = if collection.local {
            self.absolute(&format!(
                "ap/users/{}/collections/{}",
                collection.account.id, collection.id
            ))
        } else {
            collection.stored_uri.clone().unwrap_or_default()
        };
        let url = if collection.local {
            Some(self.absolute(&format!("collections/{}", collection.id)))
        } else {
            collection.stored_url.clone()
        };
        Ok(RestCollection {
            id: DecimalId::new(collection.id),
            uri,
            name: collection.name.clone(),
            description,
            language: collection.language.clone(),
            account_id: DecimalId::new(collection.account.id),
            local: collection.local,
            sensitive: collection.sensitive,
            discoverable: collection.discoverable,
            url,
            item_count: collection.items.len(),
            created_at: ApiDateTime::new(collection.created_at),
            updated_at: ApiDateTime::new(collection.updated_at),
            tag: collection.tag.as_ref().map(|tag| self.shallow_tag(tag)),
            items: collection
                .items
                .iter()
                .map(|item| RestCollectionItem {
                    id: DecimalId::new(item.id),
                    state: collection_item_state(item.state).to_owned(),
                    created_at: ApiDateTime::new(item.created_at),
                    account_id: matches!(item.state, 0 | 1)
                        .then(|| item.account_id.map(DecimalId::new))
                        .flatten(),
                })
                .collect(),
        })
    }

    pub fn collection_with_accounts(
        &self,
        collection: &CollectionProjection,
    ) -> Result<RestCollectionWithAccounts, RestError> {
        let mut accounts = vec![self.account(&collection.account)?];
        accounts.extend(
            collection
                .item_accounts
                .iter()
                .map(|account| self.account(account))
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(RestCollectionWithAccounts {
            collection: self.collection(collection)?,
            accounts,
        })
    }

    #[must_use]
    pub fn filter(&self, filter: &FilterProjection) -> RestFilter {
        Self::filter_with_rules(filter, true)
    }

    fn filter_with_rules(filter: &FilterProjection, rules: bool) -> RestFilter {
        RestFilter {
            id: DecimalId::new(filter.id),
            title: filter.title.clone(),
            context: filter.context.clone(),
            expires_at: filter.expires_at.map(ApiDateTime::new),
            filter_action: match filter.action {
                0 => "warn",
                1 => "hide",
                _ => "blur",
            }
            .to_owned(),
            keywords: rules.then(|| {
                filter
                    .keywords
                    .iter()
                    .map(|keyword| RestFilterKeyword {
                        id: DecimalId::new(keyword.id),
                        keyword: keyword.keyword.clone(),
                        whole_word: keyword.whole_word,
                    })
                    .collect()
            }),
            statuses: rules.then(|| {
                filter
                    .statuses
                    .iter()
                    .map(|status| RestFilterStatus {
                        id: DecimalId::new(status.id),
                        status_id: DecimalId::new(status.status_id),
                    })
                    .collect()
            }),
        }
    }

    fn filter_result(result: &FilterResultProjection) -> RestFilterResult {
        RestFilterResult {
            filter: Self::filter_with_rules(&result.filter, false),
            keyword_matches: result.keyword_matches.clone(),
            status_matches: result
                .status_matches
                .as_ref()
                .map(|matches| matches.iter().copied().map(DecimalId::new).collect()),
        }
    }

    #[must_use]
    pub fn marker(&self, marker: &MarkerProjection) -> RestMarker {
        RestMarker {
            last_read_id: DecimalId::new(marker.last_read_id),
            version: marker.version,
            updated_at: ApiDateTime::new(marker.updated_at),
        }
    }

    #[must_use]
    pub fn list(&self, list: &ListProjection) -> RestList {
        RestList {
            id: DecimalId::new(list.id),
            title: list.title.clone(),
            replies_policy: match list.replies_policy {
                1 => "followed",
                2 => "none",
                _ => "list",
            }
            .to_owned(),
            exclusive: list.exclusive,
        }
    }

    pub fn featured_tag(&self, tag: &FeaturedTagProjection) -> RestFeaturedTag {
        let account = tag.domain.as_deref().map_or_else(
            || format!("@{}", tag.username),
            |domain| format!("@{}@{}", tag.username, idna::domain_to_unicode(domain).0),
        );
        RestFeaturedTag {
            id: DecimalId::new(tag.id),
            name: tag.name.clone(),
            url: self.absolute(&encode_url_path(&format!(
                "{account}/tagged/{}",
                tag.tag_name
            ))),
            statuses_count: tag.statuses_count.to_string(),
            last_status_at: tag.last_status_at.map(ApiDate::new),
        }
    }

    pub fn notification(
        &self,
        notification: &NotificationProjection,
        supported_types: Option<&[String]>,
    ) -> Result<RestNotification, RestError> {
        let notification_type = notification.notification_type.raw();
        let status_type = matches!(
            notification_type,
            "mention"
                | "status"
                | "reblog"
                | "favourite"
                | "poll"
                | "update"
                | "quote"
                | "quoted_update"
        );
        let collection_type = matches!(
            notification_type,
            "added_to_collection" | "collection_update"
        );
        let status = if status_type {
            Some(
                notification
                    .status
                    .as_ref()
                    .map(|status| self.status(status, StatusShape::Full).map(Box::new))
                    .transpose()?,
            )
        } else {
            None
        };
        let collection = if collection_type {
            Some(
                notification
                    .collection
                    .as_ref()
                    .map(|collection| self.collection(collection))
                    .transpose()?,
            )
        } else {
            None
        };
        Ok(RestNotification {
            id: DecimalId::new(notification.id),
            notification_type: notification_type.to_owned(),
            created_at: ApiDateTime::new(notification.created_at),
            group_key: notification
                .group_key
                .clone()
                .unwrap_or_else(|| format!("ungrouped-{}", notification.id)),
            account: self.account(&notification.account)?,
            filtered: notification.filtered.then_some(true),
            fallback: self.notification_fallback(
                notification,
                std::slice::from_ref(&notification.account),
                supported_types,
            )?,
            status,
            report: if notification_type == "admin.report" {
                notification
                    .report
                    .as_ref()
                    .map(|report| self.report(report))
                    .transpose()?
            } else {
                None
            },
            event: if notification_type == "severed_relationships" {
                notification
                    .event
                    .as_ref()
                    .map(Self::severance_event)
                    .transpose()?
            } else {
                None
            },
            moderation_warning: if notification_type == "moderation_warning" {
                notification
                    .moderation_warning
                    .as_ref()
                    .map(|warning| self.account_warning(warning))
                    .transpose()?
            } else {
                None
            },
            collection,
        })
    }

    fn severance_event(event: &SeveranceEventProjection) -> Result<RestSeveranceEvent, RestError> {
        Ok(RestSeveranceEvent {
            id: DecimalId::new(event.id),
            event_type: match event.event_type {
                0 => "domain_block",
                1 => "user_domain_block",
                2 => "account_suspension",
                value => return Err(RestError::UnknownSeveranceEventType(value)),
            }
            .to_owned(),
            purged: event.purged,
            target_name: event.target_name.clone(),
            followers_count: event.followers_count,
            following_count: event.following_count,
            created_at: ApiDateTime::new(event.created_at),
        })
    }

    fn account_warning(
        &self,
        warning: &AccountWarningProjection,
    ) -> Result<RestAccountWarning, RestError> {
        Ok(RestAccountWarning {
            id: DecimalId::new(warning.id),
            action: match warning.action {
                0 => "none",
                1_000 => "disable",
                1_250 => "mark_statuses_as_sensitive",
                1_500 => "delete_statuses",
                2_000 => "sensitive",
                3_000 => "silence",
                4_000 => "suspend",
                value => return Err(RestError::UnknownAccountWarningAction(value)),
            }
            .to_owned(),
            text: warning.text.clone(),
            status_ids: warning
                .status_ids
                .as_ref()
                .map(|ids| ids.iter().copied().map(DecimalId::new).collect::<Vec<_>>()),
            created_at: ApiDateTime::new(warning.created_at),
            target_account: warning
                .target_account
                .as_ref()
                .map(|account| self.account(account))
                .transpose()?,
            appeal: warning.appeal.as_ref().map(|appeal| RestAppeal {
                text: appeal.text.clone(),
                state: if appeal.approved {
                    "approved"
                } else if appeal.rejected {
                    "rejected"
                } else {
                    "pending"
                }
                .to_owned(),
            }),
        })
    }

    fn report(&self, report: &ReportProjection) -> Result<RestReport, RestError> {
        Ok(RestReport {
            id: DecimalId::new(report.id),
            action_taken: report.action_taken_at.is_some(),
            action_taken_at: report.action_taken_at.map(ApiDateTime::new),
            category: match report.category {
                0 => "other",
                1_000 => "spam",
                1_500 => "legal",
                2_000 => "violation",
                value => return Err(RestError::UnknownReportCategory(value)),
            }
            .to_owned(),
            comment: report.comment.clone(),
            forwarded: report.forwarded,
            created_at: ApiDateTime::new(report.created_at),
            status_ids: report
                .status_ids
                .iter()
                .copied()
                .map(DecimalId::new)
                .collect(),
            rule_ids: report
                .rule_ids
                .as_ref()
                .map(|ids| ids.iter().copied().map(DecimalId::new).collect::<Vec<_>>()),
            collection_ids: report
                .collection_ids
                .iter()
                .copied()
                .map(DecimalId::new)
                .collect(),
            target_account: self.account(&report.target_account)?,
        })
    }

    fn notification_fallback(
        &self,
        notification: &NotificationProjection,
        sample_accounts: &[AccountProjection],
        supported_types: Option<&[String]>,
    ) -> Result<Option<RestFallback>, RestError> {
        let Some(supported_types) = supported_types else {
            return Ok(None);
        };
        let notification_type = notification.notification_type.raw();
        if notification_baseline(notification_type)
            || supported_types.iter().any(|kind| kind == notification_type)
        {
            return Ok(None);
        }
        if (notification_type == "severed_relationships" && notification.event.is_none())
            || (notification_type == "admin.report" && notification.report.is_none())
            || (matches!(
                notification_type,
                "added_to_collection" | "collection_update"
            ) && notification.collection.is_none())
        {
            return Ok(None);
        }
        let account = sample_accounts.first().unwrap_or(&notification.account);
        let mention = self.mention_html(account)?;
        let sign_in = |url: String| {
            format!(
                "<a href=\"{}\">Sign in to the Mastodon web app</a>",
                escape_html(&url)
            )
        };
        let (title, summary) = match notification_type {
            "severed_relationships" => {
                let event = notification.event.as_ref().expect("checked above");
                (
                    Some(format!("Lost connections with {}", event.target_name)),
                    Some(format!(
                        "An admin from {} has suspended {}, which means you can no longer receive updates from them or interact with them. {} to retrieve a list of the lost relationships.",
                        self.local_domain,
                        event.target_name,
                        sign_in(self.absolute("severed_relationships"))
                    )),
                )
            }
            "moderation_warning" => {
                let warning = notification.moderation_warning.as_ref();
                (
                    Some("You have received a moderation warning.".to_owned()),
                    Some(format!(
                        "You're on an app that does not support the most recent version of Mastodon. {}.",
                        sign_in(self.absolute(&format!(
                            "disputes/strikes/{}",
                            warning.map_or(0, |warning| warning.id)
                        )))
                    )),
                )
            }
            "admin.sign_up" => {
                let title = match sample_accounts.len() {
                    0 | 1 => format!("{mention} signed up"),
                    2 => format!("{mention} and one other signed up"),
                    count => format!("{mention} and {} others signed up", count - 1),
                };
                (Some(title), Some(self.generic_fallback_summary()))
            }
            "admin.report" => {
                let report = notification.report.as_ref().expect("checked above");
                let name = account.domain.clone().unwrap_or(mention);
                (
                    Some(format!(
                        "{name} reported {}",
                        self.mention_html(&report.target_account)?
                    )),
                    Some(self.generic_fallback_summary()),
                )
            }
            "added_to_collection" => (
                Some(format!("{mention} added you to a collection")),
                Some(self.generic_fallback_summary()),
            ),
            "collection_update" => (
                Some(format!(
                    "{} updated a collection you are in",
                    pretty_acct(account)
                )),
                Some(self.generic_fallback_summary()),
            ),
            _ => (None, None),
        };
        Ok(Some(RestFallback {
            title,
            summary,
            description: None,
        }))
    }

    fn generic_fallback_summary(&self) -> String {
        format!(
            "You're on an app that does not support the most recent version of Mastodon. <a href=\"{}\">Sign in to the Mastodon web app</a> for full functionality.",
            escape_html(self.origin.as_str())
        )
    }

    fn mention_html(&self, account: &AccountProjection) -> Result<String, RestError> {
        Ok(format!(
            "<span class=\"h-card\" translate=\"no\"><a href=\"{}\" class=\"u-url mention\">@<span>{}</span></a></span>",
            escape_html(&self.account_url(account)?),
            escape_html(&account.username)
        ))
    }

    pub fn grouped_notifications(
        &self,
        grouped: &GroupedNotificationsProjection,
        partial_avatars: bool,
        supported_types: Option<&[String]>,
    ) -> Result<RestGroupedNotifications, RestError> {
        let mut full_account_ids = std::collections::BTreeSet::new();
        let full_accounts = if partial_avatars {
            grouped
                .groups
                .iter()
                .filter_map(|group| group.sample_accounts.first())
                .filter(|account| full_account_ids.insert(account.id))
                .collect::<Vec<_>>()
        } else {
            grouped
                .accounts
                .iter()
                .inspect(|account| {
                    full_account_ids.insert(account.id);
                })
                .collect::<Vec<_>>()
        };
        let partial_accounts = if partial_avatars {
            let mut partial_account_ids = std::collections::BTreeSet::new();
            Some(
                grouped
                    .groups
                    .iter()
                    .flat_map(|group| group.sample_accounts.iter().skip(1))
                    .filter(|account| {
                        !full_account_ids.contains(&account.id)
                            && partial_account_ids.insert(account.id)
                    })
                    .map(|account| self.partial_account(account))
                    .collect::<Result<_, _>>()?,
            )
        } else {
            None
        };
        Ok(RestGroupedNotifications {
            accounts: full_accounts
                .iter()
                .map(|account| self.account(account))
                .collect::<Result<_, _>>()?,
            statuses: grouped
                .statuses
                .iter()
                .map(|status| self.status(status, StatusShape::Full))
                .collect::<Result<_, _>>()?,
            notification_groups: grouped
                .groups
                .iter()
                .map(|group| self.notification_group(group, supported_types))
                .collect::<Result<_, _>>()?,
            partial_accounts,
        })
    }

    fn partial_account(
        &self,
        account: &AccountProjection,
    ) -> Result<RestPartialAccount, RestError> {
        let account = self.account(account)?;
        Ok(RestPartialAccount {
            id: account.id,
            acct: account.acct,
            locked: account.locked,
            bot: account.bot,
            url: account.url,
            avatar: account.avatar,
            avatar_static: account.avatar_static,
            avatar_description: account.avatar_description,
        })
    }

    fn notification_group(
        &self,
        group: &NotificationGroupProjection,
        supported_types: Option<&[String]>,
    ) -> Result<RestNotificationGroup, RestError> {
        let notification = &group.notification;
        let notification_type = notification.notification_type.raw();
        let status_type = matches!(
            notification_type,
            "mention"
                | "status"
                | "reblog"
                | "favourite"
                | "poll"
                | "update"
                | "quote"
                | "quoted_update"
        );
        let collection_type = matches!(
            notification_type,
            "added_to_collection" | "collection_update"
        );
        Ok(RestNotificationGroup {
            group_key: group.group_key.clone(),
            notifications_count: group.notifications_count,
            notification_type: notification_type.to_owned(),
            most_recent_notification_id: group.most_recent_notification_id,
            page_min_id: Some(DecimalId::new(group.page_min_id)),
            page_max_id: Some(DecimalId::new(group.most_recent_notification_id)),
            latest_page_notification_at: Some(ApiDateTime::new(group.latest_page_notification_at)),
            fallback: self.notification_fallback(
                notification,
                &group.sample_accounts,
                supported_types,
            )?,
            sample_account_ids: group
                .sample_accounts
                .iter()
                .map(|account| DecimalId::new(account.id))
                .collect(),
            status_id: status_type.then(|| {
                notification
                    .status
                    .as_ref()
                    .map(|status| DecimalId::new(status.id))
            }),
            report: if notification_type == "admin.report" {
                notification
                    .report
                    .as_ref()
                    .map(|report| self.report(report))
                    .transpose()?
            } else {
                None
            },
            event: if notification_type == "severed_relationships" {
                notification
                    .event
                    .as_ref()
                    .map(Self::severance_event)
                    .transpose()?
            } else {
                None
            },
            moderation_warning: if notification_type == "moderation_warning" {
                notification
                    .moderation_warning
                    .as_ref()
                    .map(|warning| self.account_warning(warning))
                    .transpose()?
            } else {
                None
            },
            annual_report: (notification_type == "annual_report").then(|| RestAnnualReport {
                year: notification
                    .annual_report_year
                    .unwrap_or_default()
                    .to_string(),
            }),
            collection: if collection_type {
                Some(
                    notification
                        .collection
                        .as_ref()
                        .map(|collection| self.collection(collection))
                        .transpose()?,
                )
            } else {
                None
            },
        })
    }

    pub fn status(
        &self,
        status: &StatusProjection,
        shape: StatusShape,
    ) -> Result<RestStatus, RestError> {
        let account = self.account(&status.account)?;
        let mention_urls = status
            .mentions
            .iter()
            .map(|mention| self.account_url(&mention.account))
            .collect::<Result<Vec<_>, _>>()?;
        let mention_targets = status
            .mentions
            .iter()
            .zip(&mention_urls)
            .map(|(mention, url)| mention.account.mention_target(url))
            .collect::<Vec<_>>();
        let quote_url = status
            .quote
            .as_ref()
            .and_then(|quote| quote.target_link.as_ref())
            .and_then(|target| self.quote_target_url(target));
        let formatter = HtmlFormatter::new(self.origin, self.local_domain);
        let content = if status.local {
            formatter
                .local_text(&status.text, &mention_targets, quote_url.as_deref())
                .into_string()
        } else {
            formatter.remote_fragment(&status.text).into_string()
        };
        let viewer = status.viewer.as_ref();
        let own_status = viewer.is_some_and(|viewer| viewer.viewer_account_id == status.account.id);
        let quote = status
            .quote
            .as_ref()
            .map(|quote| self.quote(quote, shape))
            .transpose()?;
        Ok(RestStatus {
            id: DecimalId::new(status.id),
            created_at: ApiDateTime::new(status.created_at),
            in_reply_to_id: status.in_reply_to_id.map(DecimalId::new),
            in_reply_to_account_id: status.in_reply_to_account_id.map(DecimalId::new),
            sensitive: status.sensitive || (!own_status && status.account.sensitized),
            spoiler_text: status.spoiler_text.clone(),
            visibility: rest_visibility(status.visibility)?.to_owned(),
            language: status.language.clone(),
            uri: self.status_uri(status)?,
            url: self.status_url(status),
            replies_count: status.replies_count,
            reblogs_count: status.reblogs_count,
            favourites_count: status.favourites_count,
            quotes_count: status.quotes_count,
            edited_at: status.edited_at.map(ApiDateTime::new),
            favourited: viewer.map(|viewer| viewer.favourited),
            reblogged: viewer.map(|viewer| viewer.reblogged),
            muted: viewer.map(|viewer| viewer.muted),
            bookmarked: viewer.map(|viewer| viewer.bookmarked),
            pinned: viewer.and_then(|viewer| viewer.pinned),
            filtered: viewer
                .map(|viewer| viewer.filtered.iter().map(Self::filter_result).collect()),
            content: (shape != StatusShape::Source).then_some(content),
            text: (shape == StatusShape::Source).then(|| status.text.clone()),
            reblog: status
                .reblog
                .as_deref()
                .map(|reblog| self.status(reblog, StatusShape::Full).map(Box::new))
                .transpose()?,
            application: status.show_application.then(|| {
                status
                    .application
                    .as_ref()
                    .map(|application| RestApplication {
                        name: application.name.clone(),
                        website: application
                            .website
                            .as_deref()
                            .filter(|website| !website.is_empty())
                            .map(str::to_owned),
                    })
            }),
            account,
            media_attachments: status
                .media_attachments
                .iter()
                .map(|media| self.media_attachment(media))
                .collect(),
            mentions: status
                .mentions
                .iter()
                .map(|mention| self.mention(mention))
                .collect::<Result<_, _>>()?,
            tags: status
                .tags
                .iter()
                .map(|tag| self.shallow_tag(tag))
                .collect(),
            emojis: status
                .emojis
                .iter()
                .map(|emoji| self.custom_emoji(emoji))
                .collect(),
            tagged_collections: status
                .tagged_collections
                .iter()
                .map(|collection| self.collection(collection))
                .collect::<Result<_, _>>()?,
            quote,
            card: status
                .card
                .as_ref()
                .map(|card| self.preview_card(card, viewer.map(|viewer| viewer.viewer_account_id)))
                .transpose()?,
            poll: status.poll.as_ref().map(|poll| self.poll(poll)),
            quote_approval: RestQuoteApproval {
                automatic: status.quote_automatic.clone(),
                manual: status.quote_manual.clone(),
                current_user: status.quote_current_user.clone(),
            },
        })
    }

    pub fn status_context(
        &self,
        context: &StatusContextProjection,
    ) -> Result<RestStatusContext, RestError> {
        Ok(RestStatusContext {
            ancestors: context
                .ancestors
                .iter()
                .map(|status| self.status(status, StatusShape::Full))
                .collect::<Result<_, _>>()?,
            descendants: context
                .descendants
                .iter()
                .map(|status| self.status(status, StatusShape::Full))
                .collect::<Result<_, _>>()?,
        })
    }

    #[must_use]
    pub fn media_attachment(&self, media: &MediaAttachmentProjection) -> RestMediaAttachment {
        let not_processed = media.processing.is_some_and(|processing| processing != 2);
        let remote = !rails_blank(&media.remote_url);
        let needs_proxy = media.discarded || (media.file_name.is_none() && remote);
        let url = if not_processed {
            None
        } else if needs_proxy {
            Some(self.absolute(&format!("media_proxy/{}/original", media.id)))
        } else {
            media.file_name.as_ref().and_then(|file_name| {
                self.paperclip_media_url(
                    &PaperclipMetadata {
                        attachment: PaperclipAttachment::MediaFile,
                        id: media.id,
                        remote,
                        storage_schema_version: media.file_storage_schema_version,
                        file_name: file_name.clone(),
                        content_type: media.file_content_type.clone(),
                        variant: None,
                    },
                    "original",
                )
            })
        };
        let preview_url = if needs_proxy {
            Some(self.absolute(&format!("media_proxy/{}/small", media.id)))
        } else if let Some(file_name) = &media.thumbnail_file_name {
            self.paperclip_media_url(
                &PaperclipMetadata {
                    attachment: PaperclipAttachment::MediaThumbnail,
                    id: media.id,
                    remote,
                    storage_schema_version: media.thumbnail_storage_schema_version,
                    file_name: file_name.clone(),
                    content_type: None,
                    variant: None,
                },
                "original",
            )
        } else {
            media.file_name.as_ref().and_then(|file_name| {
                self.paperclip_media_url(
                    &PaperclipMetadata {
                        attachment: PaperclipAttachment::MediaFile,
                        id: media.id,
                        remote,
                        storage_schema_version: media.file_storage_schema_version,
                        file_name: file_name.clone(),
                        content_type: media.file_content_type.clone(),
                        variant: None,
                    },
                    "small",
                )
            })
        };
        RestMediaAttachment {
            id: DecimalId::new(media.id),
            media_type: match media.media_type {
                0 => "image",
                1 => "gifv",
                2 => "video",
                4 => "audio",
                _ => "unknown",
            }
            .to_owned(),
            url,
            preview_url,
            remote_url: remote.then(|| media.remote_url.clone()),
            preview_remote_url: media
                .thumbnail_remote_url
                .as_deref()
                .filter(|url| !rails_blank(url))
                .map(str::to_owned),
            text_url: (!remote && media.shortcode.is_some())
                .then(|| self.absolute(&format!("media/{}/", media.id))),
            meta: media.meta.clone(),
            description: media.description.clone(),
            blurhash: media.blurhash.clone(),
        }
    }

    pub fn mention(&self, mention: &MentionProjection) -> Result<RestMention, RestError> {
        Ok(RestMention {
            id: DecimalId::new(mention.account.id),
            username: mention.account.username.clone(),
            url: self.account_url(&mention.account)?,
            acct: pretty_acct(&mention.account),
        })
    }

    #[must_use]
    pub fn tag(&self, tag: &TagProjection) -> RestTag {
        RestTag {
            id: DecimalId::new(tag.id),
            name: tag.display_name.clone().unwrap_or_else(|| tag.name.clone()),
            url: self.tag_url(&tag.name),
            history: tag
                .history
                .iter()
                .map(|history| RestTagHistory {
                    day: history.day.clone(),
                    accounts: history.accounts.clone(),
                    uses: history.uses.clone(),
                })
                .collect(),
            following: tag.following,
            featuring: tag.featuring,
        }
    }

    #[must_use]
    pub fn poll(&self, poll: &PollProjection) -> RestPoll {
        RestPoll {
            id: DecimalId::new(poll.id),
            expires_at: poll.expires_at.map(ApiDateTime::new),
            expired: poll
                .expires_at
                .is_some_and(|expires_at| self.now >= expires_at),
            multiple: poll.multiple,
            votes_count: poll.votes_count,
            voters_count: poll.voters_count,
            options: poll
                .options
                .iter()
                .map(|option| RestPollOption {
                    title: option.title.clone(),
                    votes_count: option.votes_count,
                })
                .collect(),
            emojis: poll
                .emojis
                .iter()
                .map(|emoji| self.custom_emoji(emoji))
                .collect(),
            voted: poll.voted,
            own_votes: poll.own_votes.clone(),
        }
    }

    fn account_with_depth(
        &self,
        account: &AccountProjection,
        include_moved: bool,
    ) -> Result<RestAccount, RestError> {
        let unavailable = account.suspended;
        let formatter = HtmlFormatter::new(self.origin, self.local_domain);
        let profile_mention_urls = account
            .profile_mentions
            .iter()
            .map(|mention| self.account_url(mention))
            .collect::<Result<Vec<_>, _>>()?;
        let profile_mentions = account
            .profile_mentions
            .iter()
            .zip(&profile_mention_urls)
            .map(|(mention, url)| mention.mention_target(url))
            .collect::<Vec<_>>();
        let note = if unavailable {
            String::new()
        } else if account.local() {
            formatter
                .local_profile_text(&account.note, &profile_mentions)
                .into_string()
        } else {
            formatter.remote_fragment(&account.note).into_string()
        };
        let fields = if unavailable {
            Vec::new()
        } else {
            account
                .fields
                .iter()
                .map(|field| RestAccountField {
                    name: field.name.clone(),
                    value: if account.local() {
                        formatter
                            .local_inline(&field.value, &profile_mentions)
                            .into_string()
                    } else {
                        formatter.remote_fragment(&field.value).into_string()
                    },
                    verified_at: field.verified_at.map(ApiDateTime::new),
                })
                .collect()
        };
        let avatar = if unavailable {
            self.absolute(DEFAULT_AVATAR)
        } else {
            account.avatar_file_name.as_deref().map_or_else(
                || self.absolute(DEFAULT_AVATAR),
                |file_name| {
                    self.paperclip_media_url(
                        &PaperclipMetadata {
                            attachment: PaperclipAttachment::AccountAvatar,
                            id: account.id,
                            remote: !account.local(),
                            storage_schema_version: account.avatar_storage_schema_version,
                            file_name: file_name.to_owned(),
                            content_type: account.avatar_content_type.clone(),
                            variant: None,
                        },
                        "original",
                    )
                    .unwrap_or_else(|| self.absolute(DEFAULT_AVATAR))
                },
            )
        };
        let avatar_static = if unavailable {
            self.absolute(DEFAULT_AVATAR)
        } else if account.avatar_content_type.as_deref() == Some("image/gif") {
            account.avatar_file_name.as_deref().map_or_else(
                || self.absolute(DEFAULT_AVATAR),
                |file_name| {
                    self.paperclip_media_url(
                        &PaperclipMetadata {
                            attachment: PaperclipAttachment::AccountAvatar,
                            id: account.id,
                            remote: !account.local(),
                            storage_schema_version: account.avatar_storage_schema_version,
                            file_name: file_name.to_owned(),
                            content_type: account.avatar_content_type.clone(),
                            variant: None,
                        },
                        "static",
                    )
                    .unwrap_or_else(|| self.absolute(DEFAULT_AVATAR))
                },
            )
        } else {
            avatar.clone()
        };
        let header = if unavailable {
            self.absolute(DEFAULT_HEADER)
        } else {
            account.header_file_name.as_deref().map_or_else(
                || self.absolute(DEFAULT_HEADER),
                |file_name| {
                    self.paperclip_media_url(
                        &PaperclipMetadata {
                            attachment: PaperclipAttachment::AccountHeader,
                            id: account.id,
                            remote: !account.local(),
                            storage_schema_version: account.header_storage_schema_version,
                            file_name: file_name.to_owned(),
                            content_type: account.header_content_type.clone(),
                            variant: None,
                        },
                        "original",
                    )
                    .unwrap_or_else(|| self.absolute(DEFAULT_HEADER))
                },
            )
        };
        let header_static = if unavailable {
            self.absolute(DEFAULT_HEADER)
        } else if account.header_content_type.as_deref() == Some("image/gif") {
            account.header_file_name.as_deref().map_or_else(
                || self.absolute(DEFAULT_HEADER),
                |file_name| {
                    self.paperclip_media_url(
                        &PaperclipMetadata {
                            attachment: PaperclipAttachment::AccountHeader,
                            id: account.id,
                            remote: !account.local(),
                            storage_schema_version: account.header_storage_schema_version,
                            file_name: file_name.to_owned(),
                            content_type: account.header_content_type.clone(),
                            variant: None,
                        },
                        "static",
                    )
                    .unwrap_or_else(|| self.absolute(DEFAULT_HEADER))
                },
            )
        } else {
            header.clone()
        };
        let created_date = account
            .created_at
            .date()
            .and_hms_opt(0, 0, 0)
            .expect("midnight is a valid time");
        Ok(RestAccount {
            id: DecimalId::new(account.id),
            username: account.username.clone(),
            acct: pretty_acct(account),
            display_name: if unavailable {
                String::new()
            } else {
                account.display_name.clone()
            },
            locked: !unavailable && account.locked,
            bot: !unavailable
                && matches!(
                    account.actor_type.as_deref(),
                    Some("Application" | "Service")
                ),
            discoverable: if unavailable {
                Some(false)
            } else {
                account.discoverable
            },
            indexable: !unavailable && account.indexable,
            group: account.actor_type.as_deref() == Some("Group"),
            created_at: ApiDateTime::new(created_date),
            note,
            url: self.account_url(account)?,
            uri: self.account_uri(account),
            avatar,
            avatar_static,
            avatar_description: if unavailable {
                String::new()
            } else {
                account.avatar_description.clone()
            },
            header,
            header_static,
            header_description: if unavailable {
                String::new()
            } else {
                account.header_description.clone()
            },
            followers_count: account.followers_count,
            following_count: account.following_count,
            statuses_count: account.statuses_count,
            last_status_at: account.last_status_at.map(ApiDate::new),
            hide_collections: account.hide_collections,
            show_media: account.show_media,
            show_media_replies: account.show_media_replies,
            show_featured: account.show_featured,
            moved: if include_moved && !unavailable {
                account
                    .moved
                    .as_deref()
                    .map(|moved| self.account_with_depth(moved, false).map(Box::new))
                    .transpose()?
            } else {
                None
            },
            emojis: if unavailable {
                Vec::new()
            } else {
                account
                    .emojis
                    .iter()
                    .map(|emoji| self.custom_emoji(emoji))
                    .collect()
            },
            suspended: unavailable.then_some(true),
            limited: account.limited.then_some(true),
            noindex: account.local().then_some(account.noindex.unwrap_or(false)),
            memorial: account.memorial.then_some(true),
            feature_approval: RestFeatureApproval {
                automatic: account.feature_automatic.clone(),
                manual: account.feature_manual.clone(),
                current_user: account.feature_current_user.clone(),
            },
            email_subscriptions: account.email_subscriptions,
            roles: account.roles.as_ref().map(|roles| {
                if unavailable {
                    Vec::new()
                } else {
                    roles
                        .iter()
                        .map(|role| RestAccountRole {
                            id: DecimalId::new(role.id),
                            name: role.name.clone(),
                            color: role.color.clone(),
                        })
                        .collect()
                }
            }),
            fields,
        })
    }

    fn quote(
        &self,
        quote: &QuoteProjection,
        parent_shape: StatusShape,
    ) -> Result<RestQuotePayload, RestError> {
        let state = if quote.accepted {
            match quote.target_access {
                QuoteTargetAccess::Deleted => "deleted",
                QuoteTargetAccess::Unauthorized => "unauthorized",
                QuoteTargetAccess::Visible => &quote.state,
            }
        } else {
            &quote.state
        }
        .to_owned();
        if parent_shape == StatusShape::Shallow {
            return Ok(RestQuotePayload::Shallow(RestShallowQuote {
                state,
                quoted_status_id: (quote.target_access == QuoteTargetAccess::Visible
                    && quote.accepted
                    && quote.target_serializable)
                    .then_some(quote.quoted_status_id)
                    .flatten()
                    .map(DecimalId::new),
            }));
        }
        let quoted_status = if quote.target_access == QuoteTargetAccess::Visible
            && (quote.accepted || parent_shape == StatusShape::Source)
            && quote
                .quoted_status
                .as_ref()
                .is_some_and(|status| status.reblog.is_none())
        {
            quote
                .quoted_status
                .as_deref()
                .map(|status| self.status(status, StatusShape::Shallow).map(Box::new))
                .transpose()?
        } else {
            None
        };
        Ok(RestQuotePayload::Full(RestQuote {
            state,
            quoted_status,
        }))
    }

    fn shallow_tag(&self, tag: &TagProjection) -> RestShallowTag {
        RestShallowTag {
            name: tag.name.clone(),
            url: self.tag_url(&tag.name),
        }
    }

    fn custom_emoji(&self, emoji: &CustomEmojiProjection) -> RestCustomEmoji {
        let metadata = PaperclipMetadata {
            attachment: PaperclipAttachment::CustomEmojiImage,
            id: emoji.id,
            remote: emoji.domain.is_some(),
            storage_schema_version: emoji.storage_schema_version,
            file_name: emoji.file_name.clone(),
            content_type: None,
            variant: None,
        };
        RestCustomEmoji {
            shortcode: emoji.shortcode.clone(),
            url: self
                .paperclip_media_url(&metadata, "original")
                .unwrap_or_default(),
            static_url: self
                .paperclip_media_url(&metadata, "static")
                .or_else(|| self.paperclip_media_url(&metadata, "original"))
                .unwrap_or_default(),
            visible_in_picker: emoji.visible_in_picker,
            category: emoji.category.clone(),
            featured: emoji.featured,
        }
    }

    fn preview_card(
        &self,
        card: &PreviewCardProjection,
        viewer_account_id: Option<i64>,
    ) -> Result<RestPreviewCard, RestError> {
        let has_author = !card.author_name.is_empty()
            || !card.author_url.is_empty()
            || card.author_account.is_some();
        let authors = if has_author {
            vec![RestPreviewCardAuthor {
                name: card.author_name.clone(),
                url: card.author_url.clone(),
                account: card
                    .author_account
                    .as_ref()
                    .map(|account| self.account(account))
                    .transpose()?,
            }]
        } else {
            Vec::new()
        };
        let image = card.image_file_name.as_ref().and_then(|file_name| {
            self.paperclip_media_url(
                &PaperclipMetadata {
                    attachment: PaperclipAttachment::PreviewCardImage,
                    id: card.id,
                    remote: true,
                    storage_schema_version: card.image_storage_schema_version,
                    file_name: file_name.clone(),
                    content_type: None,
                    variant: None,
                },
                "original",
            )
        });
        Ok(RestPreviewCard {
            url: card
                .original_url
                .as_deref()
                .filter(|url| !url.is_empty())
                .unwrap_or(&card.url)
                .to_owned(),
            title: card.title.clone(),
            description: card.description.clone(),
            language: card.language.clone(),
            card_type: match card.card_type {
                0 => "link",
                1 => "photo",
                2 => "video",
                3 => "rich",
                value => return Err(RestError::UnknownPreviewCardType(value)),
            }
            .to_owned(),
            author_name: card.author_name.clone(),
            author_url: card.author_url.clone(),
            provider_name: card.provider_name.clone(),
            provider_url: card.provider_url.clone(),
            html: HtmlFormatter::new(self.origin, self.local_domain)
                .oembed_fragment(&card.html)
                .into_string(),
            width: card.width,
            height: card.height,
            image,
            image_description: card.image_description.clone(),
            embed_url: card.embed_url.clone(),
            blurhash: card.blurhash.clone(),
            published_at: card.published_at.map(ApiDateTime::new),
            authors,
            missing_attribution: viewer_account_id.map(|viewer_account_id| {
                card.unverified_author_account_id == Some(viewer_account_id)
            }),
        })
    }

    fn account_url(&self, account: &AccountProjection) -> Result<String, RestError> {
        if account.local() {
            if account.id == -99 {
                return Ok(self.absolute("about/more?instance_actor=true"));
            }
            return Ok(self.absolute(&format!("@{}", account.username)));
        }
        account
            .stored_url
            .as_deref()
            .filter(|url| is_http_url(url))
            .or_else(|| is_http_url(&account.stored_uri).then_some(account.stored_uri.as_str()))
            .map(str::to_owned)
            .ok_or(RestError::InvalidRemoteUrl(account.id))
    }

    fn account_uri(&self, account: &AccountProjection) -> String {
        if !account.local() {
            return account.stored_uri.clone();
        }
        if account.id == -99 {
            return self.absolute("actor");
        }
        match account.id_scheme {
            Some(AccountIdScheme::Numeric) => self.absolute(&format!("ap/users/{}", account.id)),
            _ => self.absolute(&format!("users/{}", account.username)),
        }
    }

    fn status_uri(&self, status: &StatusProjection) -> Result<String, RestError> {
        if !status.local {
            return status
                .stored_uri
                .clone()
                .ok_or(RestError::MissingStatusUri(status.id));
        }
        let account = &status.account;
        let activity = if status.reblog.is_some() {
            "/activity"
        } else {
            ""
        };
        Ok(match account.id_scheme {
            Some(AccountIdScheme::Numeric) => self.absolute(&format!(
                "ap/users/{}/statuses/{}{activity}",
                account.id, status.id
            )),
            _ => self.absolute(&format!(
                "users/{}/statuses/{}{activity}",
                account.username, status.id,
            )),
        })
    }

    fn status_url(&self, status: &StatusProjection) -> Option<String> {
        if status.local {
            if status.reblog.is_some() {
                return self.status_uri(status).ok();
            }
            return Some(self.absolute(&format!("@{}/{}", status.account.username, status.id)));
        }
        status.stored_url.clone()
    }

    fn quote_target_url(&self, target: &QuoteTargetLinkProjection) -> Option<String> {
        if target.local {
            return Some(self.absolute(&format!("@{}/{}", target.account.username, target.id)));
        }
        target.stored_url.clone()
    }

    fn tag_url(&self, tag: &str) -> String {
        self.absolute(&format!("tags/{tag}"))
    }

    fn paperclip_media_url(&self, metadata: &PaperclipMetadata, style: &str) -> Option<String> {
        metadata
            .relative_path(style)
            .map(|path| self.join_media_root(&path))
    }

    fn join_media_root(&self, path: &str) -> String {
        let path = encode_url_path(path);
        if let Ok(root) = Url::parse(self.media_root_url) {
            let mut root = root;
            root.set_path(&format!("{}/", root.path().trim_end_matches('/')));
            return root
                .join(&path)
                .map_or_else(|_| self.media_root_url.to_owned(), |url| url.to_string());
        }
        let root = self.media_root_url.trim_matches('/');
        self.absolute(&format!("{root}/{path}"))
    }

    fn absolute(&self, path: &str) -> String {
        self.origin
            .join(path.trim_start_matches('/'))
            .map_or_else(|_| path.to_owned(), |url| url.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestError {
    UnknownStatusVisibility(i32),
    InvalidRemoteUrl(i64),
    MissingStatusUri(i64),
    UnknownPreviewCardType(i32),
    UnknownSeveranceEventType(i32),
    UnknownAccountWarningAction(i32),
    UnknownReportCategory(i32),
}

impl fmt::Display for RestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownStatusVisibility(value) => {
                write!(formatter, "status has unsupported visibility {value}")
            }
            Self::InvalidRemoteUrl(id) => write!(formatter, "remote account {id} has no safe URL"),
            Self::MissingStatusUri(id) => write!(formatter, "remote status {id} has no URI"),
            Self::UnknownPreviewCardType(value) => {
                write!(formatter, "preview card has unsupported type {value}")
            }
            Self::UnknownSeveranceEventType(value) => {
                write!(formatter, "severance event has unsupported type {value}")
            }
            Self::UnknownAccountWarningAction(value) => {
                write!(formatter, "account warning has unsupported action {value}")
            }
            Self::UnknownReportCategory(value) => {
                write!(formatter, "report has unsupported category {value}")
            }
        }
    }
}

impl std::error::Error for RestError {}

fn rest_visibility(value: i32) -> Result<&'static str, RestError> {
    match value {
        0 => Ok("public"),
        1 => Ok("unlisted"),
        2 | 4 => Ok("private"),
        3 => Ok("direct"),
        unknown => Err(RestError::UnknownStatusVisibility(unknown)),
    }
}

fn pretty_acct(account: &AccountProjection) -> String {
    account.domain.as_deref().map_or_else(
        || account.username.clone(),
        |domain| format!("{}@{}", account.username, idna::domain_to_unicode(domain).0),
    )
}

fn notification_baseline(notification_type: &str) -> bool {
    matches!(
        notification_type,
        "mention"
            | "status"
            | "reblog"
            | "follow"
            | "follow_request"
            | "favourite"
            | "poll"
            | "update"
            | "annual_report"
            | "quote"
            | "quoted_update"
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn is_http_url(value: &str) -> bool {
    Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

fn collection_item_state(value: i32) -> &'static str {
    match value {
        0 => "pending",
        1 => "accepted",
        2 => "rejected",
        _ => "revoked",
    }
}
