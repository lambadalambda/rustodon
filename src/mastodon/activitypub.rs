#![allow(clippy::missing_panics_doc, clippy::needless_pass_by_value)]

use std::fmt::Write as _;

use chrono::{NaiveDateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::paperclip::{PaperclipAttachment, PaperclipMetadata, rails_blank};

use super::records::{Account, MediaAttachment, Mention, Status};
use super::rest::{HtmlFormatter, MentionTarget};
use super::types::{AccountIdScheme, StatusVisibility};

pub const ACTIVITY_JSON: &str = "application/activity+json; charset=utf-8";
pub const JRD_JSON: &str = "application/jrd+json; charset=utf-8";
pub const XRD_XML: &str = "application/xrd+xml; charset=utf-8";
pub const ACTIVITY_STREAMS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";
pub const SECURITY_CONTEXT: &str = "https://w3id.org/security/v1";
pub const WEBFINGER_CONTEXT: &str = "https://purl.archive.org/socialweb/webfinger";
pub const PUBLIC_ADDRESS: &str = "https://www.w3.org/ns/activitystreams#Public";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomEmoji {
    pub id: i64,
    pub shortcode: String,
    pub file_name: String,
    pub content_type: Option<String>,
    pub storage_schema_version: Option<i32>,
    pub updated_at: NaiveDateTime,
}

#[must_use]
pub fn is_public_address(uri: &str) -> bool {
    matches!(uri, PUBLIC_ADDRESS | "as:Public" | "Public")
}

#[must_use]
pub fn emoji(origin: &Url, media_root_url: &str, emoji: &CustomEmoji) -> Value {
    let mut value = emoji_tag(origin, media_root_url, emoji);
    value.as_object_mut().expect("emoji is an object").insert(
        "@context".to_owned(),
        json!([ACTIVITY_STREAMS_CONTEXT, {
            "toot": "http://joinmastodon.org/ns#",
            "Emoji": "toot:Emoji"
        }]),
    );
    value
}

fn emoji_tag(origin: &Url, media_root_url: &str, emoji: &CustomEmoji) -> Value {
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::CustomEmojiImage,
        id: emoji.id,
        remote: false,
        storage_schema_version: emoji.storage_schema_version,
        file_name: emoji.file_name.clone(),
        content_type: emoji.content_type.clone(),
        variant: None,
    };
    json!({
        "id": origin.join(&format!("emojis/{}", emoji.id)).expect("origin is absolute").to_string(),
        "type": "Emoji",
        "name": format!(":{}:", emoji.shortcode),
        "updated": timestamp(emoji.updated_at),
        "icon": {
            "type": "Image",
            "mediaType": emoji.content_type,
            "url": paperclip_url(origin, media_root_url, &metadata, "original")
        }
    })
}

#[must_use]
pub fn actor_url(origin: &Url, account: &Account) -> String {
    if account.domain.is_some() && !account.uri.is_empty() {
        return account.uri.clone();
    }
    origin
        .join(&canonical_account_path(account))
        .expect("origin must be an absolute URL")
        .to_string()
}

#[must_use]
pub fn profile_url(origin: &Url, account: &Account) -> String {
    if account.domain.is_some() {
        account
            .url
            .clone()
            .unwrap_or_else(|| actor_url(origin, account))
    } else if account.id == -99 {
        origin
            .join("about/more?instance_actor=true")
            .expect("origin must be an absolute URL")
            .to_string()
    } else {
        origin
            .join(&format!("@{}", account.username))
            .expect("origin must be an absolute URL")
            .to_string()
    }
}

#[must_use]
pub fn status_url(origin: &Url, account: &Account, status: &Status) -> String {
    if account.domain.is_some() {
        return status
            .url
            .clone()
            .unwrap_or_else(|| actor_url(origin, account));
    }
    let activity_suffix = status.reblog_of_id.map_or("", |_| "/activity");
    origin
        .join(&format!(
            "@{}/{}{}",
            account.username, status.id, activity_suffix
        ))
        .expect("origin must be an absolute URL")
        .to_string()
}

#[must_use]
pub fn status_uri(origin: &Url, account: &Account, status: &Status) -> String {
    if account.domain.is_some() {
        return status
            .uri
            .clone()
            .unwrap_or_else(|| status_url(origin, account, status));
    }
    let activity_suffix = status.reblog_of_id.map_or("", |_| "/activity");
    origin
        .join(&format!(
            "{}/statuses/{}{}",
            canonical_account_path(account),
            status.id,
            activity_suffix
        ))
        .expect("origin must be an absolute URL")
        .to_string()
}

#[must_use]
pub fn local_status_uri(
    origin: &Url,
    account_id: i64,
    username: &str,
    id_scheme: Option<AccountIdScheme>,
    status_id: i64,
) -> String {
    let account_path = if id_scheme == Some(AccountIdScheme::Numeric) {
        format!("ap/users/{account_id}")
    } else {
        format!("users/{username}")
    };
    origin
        .join(&format!("{account_path}/statuses/{status_id}"))
        .expect("origin must be an absolute URL")
        .to_string()
}

#[must_use]
pub fn quote_authorization_url(origin: &Url, account: &Account, quote_id: i64) -> String {
    local_quote_authorization_url(
        origin,
        account.id,
        &account.username,
        account.id_scheme,
        quote_id,
    )
}

#[must_use]
pub fn local_quote_authorization_url(
    origin: &Url,
    account_id: i64,
    username: &str,
    id_scheme: Option<AccountIdScheme>,
    quote_id: i64,
) -> String {
    let account_path = if id_scheme == Some(AccountIdScheme::Numeric) {
        format!("ap/users/{account_id}")
    } else {
        format!("users/{username}")
    };
    origin
        .join(&format!("{account_path}/quote_authorizations/{quote_id}"))
        .expect("origin must be an absolute URL")
        .to_string()
}

fn canonical_account_path(account: &Account) -> String {
    if account.id == -99 {
        "actor".to_owned()
    } else if account.id_scheme == Some(AccountIdScheme::Numeric) {
        format!("ap/users/{}", account.id)
    } else {
        format!("users/{}", account.username)
    }
}

fn local_endpoint(origin: &Url, account: &Account, endpoint: &str) -> String {
    origin
        .join(&format!("{}/{endpoint}", canonical_account_path(account)))
        .expect("origin must be an absolute URL")
        .to_string()
}

fn endpoint(origin: &Url, account: &Account, endpoint: &str, remote: &str) -> String {
    if account.domain.is_none() {
        local_endpoint(origin, account, endpoint)
    } else {
        remote.to_owned()
    }
}

fn shared_inbox(origin: &Url, account: &Account) -> String {
    if account.domain.is_none() {
        origin
            .join("inbox")
            .expect("origin must be an absolute URL")
            .to_string()
    } else {
        account.shared_inbox_url.clone()
    }
}

#[must_use]
pub fn collection_url(origin: &Url, account: &Account, collection: &str) -> String {
    format!(
        "{}/{}",
        actor_url(origin, account).trim_end_matches('/'),
        collection
    )
}

#[must_use]
pub fn replies_url(origin: &Url, account: &Account, status: &Status) -> String {
    status_collection_url(origin, account, status, "replies")
}

#[must_use]
pub fn likes_url(origin: &Url, account: &Account, status: &Status) -> String {
    status_collection_url(origin, account, status, "likes")
}

#[must_use]
pub fn shares_url(origin: &Url, account: &Account, status: &Status) -> String {
    status_collection_url(origin, account, status, "shares")
}

fn status_collection_url(
    origin: &Url,
    account: &Account,
    status: &Status,
    collection: &str,
) -> String {
    let status_url = if account.domain.is_none() {
        origin
            .join(&format!(
                "{}/statuses/{}",
                canonical_account_path(account),
                status.id
            ))
            .expect("origin must be an absolute URL")
            .to_string()
    } else {
        status_uri(origin, account, status)
    };
    format!("{status_url}/{collection}")
}

fn actor_context() -> Value {
    json!([
        ACTIVITY_STREAMS_CONTEXT,
        SECURITY_CONTEXT,
        WEBFINGER_CONTEXT,
        {
            "toot": "http://joinmastodon.org/ns#",
            "manuallyApprovesFollowers": "as:manuallyApprovesFollowers",
            "featured": {"@id": "toot:featured", "@type": "@id"},
            "featuredTags": {"@id": "toot:featuredTags", "@type": "@id"},
            "alsoKnownAs": {"@id": "as:alsoKnownAs", "@type": "@id"},
            "movedTo": {"@id": "as:movedTo", "@type": "@id"},
            "schema": "http://schema.org#",
            "PropertyValue": "schema:PropertyValue",
            "value": "schema:value",
            "Hashtag": "as:Hashtag",
            "focalPoint": {"@container": "@list", "@id": "toot:focalPoint"},
            "discoverable": "toot:discoverable",
            "suspended": "toot:suspended",
            "indexable": "toot:indexable",
            "memorial": "toot:memorial",
            "attributionDomains": {"@id": "toot:attributionDomains", "@container": "@set"},
            "showFeatured": "toot:showFeatured",
            "showMedia": "toot:showMedia",
            "showRepliesInMedia": "toot:showRepliesInMedia",
            "gts": "https://gotosocial.org/ns#",
            "interactionPolicy": {"@id": "gts:interactionPolicy", "@type": "@id"},
            "canFeature": {"@id": "https://w3id.org/fep/7aa9#canFeature", "@type": "@id"},
            "canQuote": {"@id": "gts:canQuote", "@type": "@id"},
            "automaticApproval": {"@id": "gts:automaticApproval", "@type": "@id"},
            "manualApproval": {"@id": "gts:manualApproval", "@type": "@id"}
        }
    ])
}

fn note_context() -> Value {
    json!([
        ACTIVITY_STREAMS_CONTEXT,
        {
            "ostatus": "http://ostatus.org#",
            "atomUri": "ostatus:atomUri",
            "inReplyToAtomUri": "ostatus:inReplyToAtomUri",
            "conversation": "ostatus:conversation",
            "toot": "http://joinmastodon.org/ns#",
            "Hashtag": "as:Hashtag",
            "Emoji": "toot:Emoji",
            "blurhash": "toot:blurhash",
            "focalPoint": {"@container": "@list", "@id": "toot:focalPoint"},
            "sensitive": "as:sensitive",
            "votersCount": "toot:votersCount",
            "quote": {"@id": "https://w3id.org/fep/044f#quote", "@type": "@id"},
            "quoteUri": "http://fedibird.com/ns#quoteUri",
            "_misskey_quote": "https://misskey-hub.net/ns#_misskey_quote",
            "quoteAuthorization": {"@id": "https://w3id.org/fep/044f#quoteAuthorization", "@type": "@id"},
            "gts": "https://gotosocial.org/ns#",
            "interactionPolicy": {"@id": "gts:interactionPolicy", "@type": "@id"},
            "canFeature": {"@id": "https://w3id.org/fep/7aa9#canFeature", "@type": "@id"},
            "canQuote": {"@id": "gts:canQuote", "@type": "@id"},
            "automaticApproval": {"@id": "gts:automaticApproval", "@type": "@id"},
            "manualApproval": {"@id": "gts:manualApproval", "@type": "@id"}
        }
    ])
}

fn quote_authorization_context() -> Value {
    json!([
        ACTIVITY_STREAMS_CONTEXT,
        {
            "gts": "https://gotosocial.org/ns#",
            "QuoteAuthorization": "https://w3id.org/fep/044f#QuoteAuthorization",
            "interactingObject": {"@id": "gts:interactingObject", "@type": "@id"},
            "interactionTarget": {"@id": "gts:interactionTarget", "@type": "@id"}
        }
    ])
}

#[must_use]
pub fn quote_authorization(
    origin: &Url,
    quoted_account: &Account,
    quoted_status: &Status,
    quoting_account: &Account,
    quoting_status: &Status,
    quote_id: i64,
) -> Value {
    json!({
        "@context": quote_authorization_context(),
        "id": quote_authorization_url(origin, quoted_account, quote_id),
        "type": "QuoteAuthorization",
        "attributedTo": actor_url(origin, quoted_account),
        "interactingObject": status_uri(origin, quoting_account, quoting_status),
        "interactionTarget": status_uri(origin, quoted_account, quoted_status)
    })
}

#[derive(Debug)]
struct ProfileMention {
    username: String,
    domain: Option<String>,
    url: String,
}

fn profile_summary(origin: &Url, local_domain: &str, note: &str) -> String {
    let mentions = note
        .split_whitespace()
        .filter_map(|word| {
            let word = word.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '@' | '.' | '-' | '_')
            });
            let mention = word.strip_prefix('@')?;
            let (username, domain) = mention.split_once('@')?;
            if username.is_empty() || domain.is_empty() {
                return None;
            }
            let url = if domain.eq_ignore_ascii_case(local_domain) {
                origin.join(&format!("@{username}"))
            } else {
                Url::parse(&format!("https://{domain}/@{username}"))
            }
            .ok()?
            .to_string();
            Some(ProfileMention {
                username: username.to_owned(),
                domain: (!domain.eq_ignore_ascii_case(local_domain)).then(|| domain.to_owned()),
                url,
            })
        })
        .collect::<Vec<_>>();
    let targets = mentions
        .iter()
        .map(|mention| MentionTarget {
            username: &mention.username,
            domain: mention.domain.as_deref(),
            url: &mention.url,
        })
        .collect::<Vec<_>>();
    HtmlFormatter::new(origin, local_domain)
        .local_profile_text(note, &targets)
        .into_string()
}

#[must_use]
pub fn webfinger(
    origin: &Url,
    local_domain: &str,
    media_root_url: &str,
    limited_federation: bool,
    account: &Account,
) -> Value {
    let actor = actor_url(origin, account);
    let profile = profile_url(origin, account);
    let subject = if account.id == -99 {
        format!("acct:{local_domain}@{local_domain}")
    } else {
        format!("acct:{}@{local_domain}", account.username)
    };
    let aliases = if account.id == -99 {
        vec![Value::String(actor.clone())]
    } else {
        vec![Value::String(profile.clone()), Value::String(actor.clone())]
    };
    let mut links = vec![
        json!({
            "rel": "http://webfinger.net/rel/profile-page",
            "type": "text/html",
            "href": profile
        }),
        json!({
            "rel": "self",
            "type": "application/activity+json",
            "href": actor
        }),
        json!({
            "rel": "http://ostatus.org/schema/1.0/subscribe",
            "template": format!("{}/authorize_interaction?uri={{uri}}", origin.as_str().trim_end_matches('/'))
        }),
        json!({
            "rel": "https://w3id.org/fep/3b86/Create",
            "template": format!("{}/share?text={{content}}", origin.as_str().trim_end_matches('/'))
        }),
        json!({
            "rel": "https://w3id.org/fep/3b86/Object",
            "template": format!("{}/authorize_interaction?uri={{object}}", origin.as_str().trim_end_matches('/'))
        }),
    ];
    if !limited_federation
        && let Some(href) = avatar_url(origin, media_root_url, account)
        && let Some(content_type) = account.avatar_content_type.as_deref()
    {
        links.push(json!({
            "rel": "http://webfinger.net/rel/avatar",
            "type": content_type,
            "href": href
        }));
    }
    json!({
        "subject": subject,
        "aliases": aliases,
        "links": links
    })
}

#[must_use]
pub fn host_meta(origin: &Url, json_format: bool) -> (String, Vec<u8>) {
    let authority = origin.port().map_or_else(
        || origin.host_str().unwrap_or_default().to_owned(),
        |port| format!("{}:{port}", origin.host_str().unwrap_or_default()),
    );
    let template = format!(
        "{}://{}/.well-known/webfinger?resource={{uri}}",
        origin.scheme(),
        authority
    );
    if json_format {
        (
            "application/json; charset=utf-8".to_owned(),
            serde_json::to_vec(&json!({
                "links": [{"rel": "lrdd", "template": template}]
            }))
            .expect("host-meta JSON is serializable"),
        )
    } else {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<XRD xmlns=\"http://docs.oasis-open.org/ns/xri/xrd-1.0\"><Link rel=\"lrdd\" template=\"{template}\"/></XRD>"
        );
        (XRD_XML.to_owned(), body.into_bytes())
    }
}

#[must_use]
pub fn nodeinfo_discovery(origin: &Url) -> Value {
    json!({
        "links": [{
            "rel": "http://nodeinfo.diaspora.software/ns/schema/2.0",
            "href": origin.join("nodeinfo/2.0").expect("origin must be absolute").to_string()
        }]
    })
}

#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn nodeinfo(
    version: &str,
    title: &str,
    description: &str,
    users: i64,
    posts: i64,
    active_month: i64,
    active_halfyear: i64,
    open_registrations: bool,
) -> Value {
    json!({
        "version": "2.0",
        "software": {"name": "mastodon", "version": version},
        "protocols": ["activitypub"],
        "services": {"inbound": [], "outbound": []},
        "usage": {
            "users": {"total": users, "activeMonth": active_month, "activeHalfyear": active_halfyear},
            "localPosts": posts
        },
        "openRegistrations": open_registrations,
        "metadata": {
            "nodeName": title,
            "nodeDescription": description
        }
    })
}

#[must_use]
pub fn actor(origin: &Url, local_domain: &str, account: &Account) -> Value {
    actor_with_media(origin, local_domain, "", account, &[], &[])
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn actor_with_media(
    origin: &Url,
    local_domain: &str,
    media_root_url: &str,
    account: &Account,
    hashtags: &[String],
    emojis: &[CustomEmoji],
) -> Value {
    let actor = actor_url(origin, account);
    let is_instance = account.id == -99;
    let actor_type = if is_instance {
        "Application"
    } else {
        match account.actor_type.as_ref().map(|value| value.0.as_str()) {
            Some("Group") => "Group",
            Some("Service" | "Application") => "Service",
            _ => "Person",
        }
    };
    let unavailable = !is_instance && account.suspended_at.is_some();
    let name = if unavailable || account.display_name.is_empty() {
        account.username.clone()
    } else {
        account.display_name.clone()
    };
    let summary = if unavailable {
        String::new()
    } else {
        profile_summary(origin, local_domain, &account.note)
    };
    let published = account
        .created_at
        .date()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid");
    let mut value = json!({
       "@context": actor_context(),
       "id": actor,
       "type": actor_type,
       "preferredUsername": account.username,
       "name": name,
       "summary": summary,
       "url": profile_url(origin, account),
       "inbox": endpoint(origin, account, "inbox", &account.inbox_url),
       "outbox": endpoint(origin, account, "outbox", &account.outbox_url),
       "followers": endpoint(origin, account, "followers", &account.followers_url),
       "following": endpoint(origin, account, "following", &account.following_url),
       "manuallyApprovesFollowers": !unavailable && account.locked,
       "discoverable": !unavailable && account.discoverable.unwrap_or(false),
       "indexable": !unavailable && account.indexable,
       "published": timestamp(published),
       "memorial": account.memorial,
       "showFeatured": account.show_featured,
       "showMedia": account.show_media,
       "showRepliesInMedia": account.show_media_replies,
       "publicKey": {
           "id": format!("{actor}#main-key"),
           "owner": actor,
        "publicKeyPem": account.public_key
        },
        "endpoints": {"sharedInbox": shared_inbox(origin, account)}
    });
    if !is_instance && !unavailable {
        let object = value.as_object_mut().expect("actor is an object");
        if let (Some(url), Some(media_type)) = (
            avatar_url(origin, media_root_url, account),
            account.avatar_content_type.as_deref(),
        ) {
            object.insert(
                "icon".to_owned(),
                profile_image(&url, media_type, &account.avatar_description),
            );
        }
        if let (Some(url), Some(media_type)) = (
            header_url(origin, media_root_url, account),
            account.header_content_type.as_deref(),
        ) {
            object.insert(
                "image".to_owned(),
                profile_image(&url, media_type, &account.header_description),
            );
        }
        object.insert(
            "attachment".to_owned(),
            Value::Array(profile_attachments(origin, local_domain, account)),
        );
        object.insert(
            "tag".to_owned(),
            Value::Array(profile_tags(origin, media_root_url, hashtags, emojis)),
        );
    }
    if is_instance {
        let object = value.as_object_mut().expect("actor is an object");
        for key in [
            "name",
            "summary",
            "discoverable",
            "indexable",
            "published",
            "memorial",
            "showFeatured",
            "showMedia",
            "showRepliesInMedia",
            "following",
            "followers",
        ] {
            object.remove(key);
        }
    } else if unavailable {
        value["suspended"] = json!(true);
    }
    value
}

/// Identifies a persisted Update version at `PostgreSQL` timestamp precision.
#[must_use]
pub fn update_activity_id(object_uri: &str, version: NaiveDateTime) -> String {
    format!(
        "{object_uri}#updates/{}",
        version.and_utc().timestamp_micros()
    )
}

#[must_use]
pub fn update_actor(
    origin: &Url,
    local_domain: &str,
    media_root_url: &str,
    account: &Account,
    hashtags: &[String],
    emojis: &[CustomEmoji],
) -> Value {
    let actor_uri = actor_url(origin, account);
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": update_activity_id(&actor_uri, account.updated_at),
        "type": "Update",
        "actor": actor_uri,
        "to": [PUBLIC_ADDRESS],
        "object": actor_with_media(origin, local_domain, media_root_url, account, hashtags, emojis)
    })
}

fn profile_image(url: &str, media_type: &str, description: &str) -> Value {
    let mut image = json!({
        "type": "Image",
        "mediaType": media_type,
        "url": url
    });
    if !description.is_empty() {
        image["summary"] = json!(description);
    }
    image
}

fn profile_attachments(origin: &Url, local_domain: &str, account: &Account) -> Vec<Value> {
    let formatter = HtmlFormatter::new(origin, local_domain);
    account
        .fields
        .as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|field| {
            let name = field.get("name")?.as_str()?.to_owned();
            let value = field.get("value")?.as_str()?;
            Some(json!({
                "type": "PropertyValue",
                "name": name,
                "value": formatter.local_profile_inline(value, &[]).into_string()
            }))
        })
        .collect()
}

fn profile_tags(
    origin: &Url,
    media_root_url: &str,
    hashtags: &[String],
    emojis: &[CustomEmoji],
) -> Vec<Value> {
    let emoji_tags = emojis
        .iter()
        .map(|emoji| emoji_tag(origin, media_root_url, emoji));
    let hashtag_tags = hashtags.iter().filter_map(|name| {
        Some(json!({
            "type": "Hashtag",
            "href": origin.join(&format!("tags/{name}")).ok()?.to_string(),
            "name": format!("#{name}")
        }))
    });
    emoji_tags.chain(hashtag_tags).collect()
}

#[must_use]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub fn note(
    origin: &Url,
    local_domain: &str,
    status: &Status,
    account: &Account,
    media_root_url: &str,
    media: &[MediaAttachment],
    mentions: &[(Mention, Account)],
    hashtags: &[(String, String)],
    emojis: &[CustomEmoji],
    quoted_url: Option<&str>,
    in_reply_to_url: Option<&str>,
    in_reply_to_atom_uri: Option<&str>,
    conversation: Option<&str>,
    quote_identifier: Option<&str>,
    quote_authorization: Option<&str>,
    replies: Option<Value>,
    favourites_count: i64,
    reblogs_count: i64,
) -> Value {
    let id = status_uri(origin, account, status);
    let url = status_url(origin, account, status);
    let atom_uri = account.domain.is_none().then(|| {
        status.uri.clone().unwrap_or_else(|| {
            format!(
                "tag:{local_domain},{}:objectId={}:objectType=Status",
                status.created_at.date(),
                status.id
            )
        })
    });
    let followers = endpoint(origin, account, "followers", &account.followers_url);
    let following = endpoint(origin, account, "following", &account.following_url);
    let quote_policy = status.quote_approval_policy.0 >> 16;
    let mut automatic_quote_approval = Vec::new();
    if quote_policy & (1 << 1) != 0 {
        automatic_quote_approval.push(Value::String(PUBLIC_ADDRESS.to_owned()));
    }
    if quote_policy & (1 << 2) != 0 {
        automatic_quote_approval.push(Value::String(followers.clone()));
    }
    if quote_policy & (1 << 3) != 0 {
        automatic_quote_approval.push(Value::String(following));
    }
    if automatic_quote_approval.is_empty() {
        automatic_quote_approval.push(Value::String(actor_url(origin, account)));
    }
    let mention_addresses = mentions
        .iter()
        .map(|(_, target)| Value::String(actor_url(origin, target)))
        .collect::<Vec<_>>();
    let (to, cc) = match status.visibility {
        super::StatusVisibility::Public => (
            vec![Value::String(PUBLIC_ADDRESS.to_owned())],
            std::iter::once(Value::String(followers.clone()))
                .chain(mention_addresses.clone())
                .collect(),
        ),
        super::StatusVisibility::Unlisted => (
            vec![Value::String(followers.clone())],
            std::iter::once(Value::String(PUBLIC_ADDRESS.to_owned()))
                .chain(mention_addresses.clone())
                .collect(),
        ),
        super::StatusVisibility::Private => {
            (vec![Value::String(followers)], mention_addresses.clone())
        }
        super::StatusVisibility::Direct | super::StatusVisibility::Limited => {
            (mention_addresses, Vec::new())
        }
        super::StatusVisibility::Unknown(_) => (Vec::new(), Vec::new()),
    };
    let mention_tags = mentions.iter().map(|(_, target)| {
        json!({
            "type": "Mention",
            "href": actor_url(origin, target),
            "name": if let Some(domain) = target.domain.as_deref() {
                format!("@{}@{domain}", target.username)
            } else {
                format!("@{}", target.username)
            }
        })
    });
    let hashtag_tags = hashtags.iter().filter_map(|(name, _display_name)| {
        let href = origin.join(&format!("tags/{name}")).ok()?.to_string();
        Some(json!({
            "type": "Hashtag",
            "href": href,
            "name": format!("#{name}")
        }))
    });
    let emoji_tags = emojis.iter().filter_map(|emoji| {
        let metadata = PaperclipMetadata {
            attachment: PaperclipAttachment::CustomEmojiImage,
            id: emoji.id,
            remote: false,
            storage_schema_version: emoji.storage_schema_version,
            file_name: emoji.file_name.clone(),
            content_type: emoji.content_type.clone(),
            variant: None,
        };
        let icon_url = paperclip_url(origin, media_root_url, &metadata, "original")?;
        let id = origin
            .join(&format!("emojis/{}", emoji.id))
            .ok()?
            .to_string();
        Some(json!({
            "id": id,
            "type": "Emoji",
            "name": format!(":{}:", emoji.shortcode),
            "updated": timestamp(emoji.updated_at),
            "icon": {
                "type": "Image",
                "mediaType": emoji.content_type,
                "url": icon_url
            }
        }))
    });
    let mention_urls = mentions
        .iter()
        .map(|(_, target)| profile_url(origin, target))
        .collect::<Vec<_>>();
    let mention_targets = mentions
        .iter()
        .zip(&mention_urls)
        .map(|((_, target), url)| MentionTarget {
            username: &target.username,
            domain: target.domain.as_deref(),
            url,
        })
        .collect::<Vec<_>>();
    let attachments = media.iter().filter_map(|attachment| {
        let media_url = media_url(origin, media_root_url, attachment)?;
        let original_metadata = attachment
            .file_meta
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("original"))
            .and_then(Value::as_object);
        let width = original_metadata
            .and_then(|metadata| metadata.get("width"))
            .and_then(Value::as_u64);
        let height = original_metadata
            .and_then(|metadata| metadata.get("height"))
            .and_then(Value::as_u64);
        let focal_point = attachment
            .file_meta
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("focus"))
            .and_then(Value::as_object)
            .and_then(|focus| {
                Some(json!([
                    focus.get("x")?.as_f64()?,
                    focus.get("y")?.as_f64()?
                ]))
            });
        let mut value = json!({
            "type": "Document",
            "mediaType": attachment.file_content_type.clone().unwrap_or_else(|| "application/octet-stream".to_owned()),
            "url": media_url,
            "name": attachment.description,
            "blurhash": attachment.blurhash
        });
        if let Some(focal_point) = focal_point {
            value["focalPoint"] = focal_point;
        }
        if let Some(width) = width {
            value["width"] = json!(width);
        }
        if let Some(height) = height {
            value["height"] = json!(height);
        }
        if let Some(url) = media_thumbnail_url(origin, media_root_url, attachment) {
            value["icon"] = json!({
                "type": "Image",
                "mediaType": attachment.thumbnail_content_type.clone(),
                "url": url,
            });
        }
        Some(value)
    });
    let formatter = HtmlFormatter::new(origin, local_domain);
    let content = formatter
        .local_text(&status.text, &mention_targets, quoted_url)
        .into_string();
    let mut value = json!({
        "@context": note_context(),
        "id": id,
        "type": "Note",
        "summary": (!status.spoiler_text.is_empty()).then_some(status.spoiler_text.clone()),
        "inReplyTo": in_reply_to_url,
        "inReplyToAtomUri": in_reply_to_atom_uri,
        "conversation": conversation,
        "context": conversation,
        "published": timestamp(status.created_at),
        "url": url,
        "attributedTo": actor_url(origin, account),
        "to": to,
        "cc": cc,
        "sensitive": status.sensitive || account.sensitized_at.is_some(),
        "interactionPolicy": {
            "canQuote": {
                "automaticApproval": automatic_quote_approval
            }
        },
        "atomUri": atom_uri,
        "content": content,
        "attachment": attachments.collect::<Vec<_>>(),
        "tag": mention_tags.chain(hashtag_tags).chain(emoji_tags).collect::<Vec<_>>()
    });
    if let Some(language) = status.language.as_deref() {
        value["contentMap"] = json!({language: value["content"].clone()});
    }
    if let Some(edited_at) = status.edited_at {
        value["updated"] = json!(timestamp(edited_at));
    }
    if let Some(quote_identifier) = quote_identifier {
        value["quote"] = json!(quote_identifier);
        value["quoteUri"] = json!(quote_identifier);
        value["_misskey_quote"] = json!(quote_identifier);
    }
    if let Some(quote_authorization) = quote_authorization {
        value["quoteAuthorization"] = json!(quote_authorization);
    }
    if let Some(replies) = replies {
        value["replies"] = replies;
    }
    if status.local == Some(true) || status.uri.is_none() {
        value["likes"] = json!({
            "id": likes_url(origin, account, status),
            "type": "Collection",
            "totalItems": favourites_count.max(0)
        });
        value["shares"] = json!({
            "id": shares_url(origin, account, status),
            "type": "Collection",
            "totalItems": reblogs_count.max(0)
        });
    }
    value
}

/// Projects a status Note into Mastodon's `ActivityPub` Question representation.
#[must_use]
pub fn question(mut value: Value, poll: &super::Poll, now: NaiveDateTime) -> Value {
    value["type"] = json!("Question");
    let show_totals = !poll.hide_totals || poll.expires_at.is_some_and(|expiry| now >= expiry);
    let options = poll
        .options
        .iter()
        .enumerate()
        .map(|(index, name)| {
            json!({
                "type": "Note",
                "name": name,
                "replies": {
                    "type": "Collection",
                    "totalItems": show_totals
                        .then(|| poll.cached_tallies.get(index).copied().unwrap_or(0))
                }
            })
        })
        .collect::<Vec<_>>();
    let key = if poll.multiple { "anyOf" } else { "oneOf" };
    value[key] = Value::Array(options);
    if let Some(expires_at) = poll.expires_at {
        value["endTime"] = json!(timestamp(expires_at));
        if now >= expires_at {
            value["closed"] = json!(timestamp(expires_at));
        }
    }
    if let Some(voters_count) = poll.voters_count {
        value["votersCount"] = json!(voters_count.max(0));
    }
    value
}

#[must_use]
pub fn create(origin: &Url, account: &Account, status: &Status, object: Value) -> Value {
    let activity_id = object["id"].as_str().map_or_else(
        || format!("{}/activity", status_uri(origin, account, status)),
        |object_id| format!("{object_id}/activity"),
    );
    json!({
        "@context": note_context(),
        "id": activity_id,
        "type": "Create",
        "actor": actor_url(origin, account),
        "published": timestamp(status.created_at),
        "to": object["to"],
        "cc": object["cc"],
        "object": object
    })
}

#[must_use]
pub fn status_activity(
    origin: &Url,
    account: &Account,
    status: &Status,
    object: Value,
    to: Value,
    cc: Value,
) -> Value {
    if status.reblog_of_id.is_some() {
        announce_with_object(
            &status_uri(origin, account, status),
            &actor_url(origin, account),
            status.created_at,
            object,
            to,
            cc,
        )
    } else {
        create(origin, account, status, object)
    }
}

#[must_use]
pub fn vote_with_uris(
    vote_uri: &str,
    actor_uri: &str,
    question_uri: &str,
    poll_actor_uri: &str,
    option: &str,
) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": format!("{vote_uri}/activity"),
        "type": "Create",
        "actor": actor_uri,
        "to": poll_actor_uri,
        "object": {
            "id": vote_uri,
            "type": "Note",
            "name": option,
            "attributedTo": actor_uri,
            "inReplyTo": question_uri,
            "to": poll_actor_uri
        }
    })
}

#[must_use]
pub fn flag_with_uris(
    report_uri: &str,
    actor_uri: &str,
    object_uris: &[String],
    content: &str,
) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": report_uri,
        "type": "Flag",
        "actor": actor_uri,
        "object": object_uris,
        "content": content
    })
}

#[must_use]
pub fn flag_delivery_logical_key(flag_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("flag", flag_uri, inbox_url)
}

#[must_use]
pub fn forward_delivery_logical_key(
    source_account_id: i64,
    activity_uri: &str,
    inbox_url: &str,
) -> String {
    let digest = Sha256::digest(format!("{activity_uri}\n{inbox_url}").as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:forward:{source_account_id}:{digest_string}")
}

#[must_use]
pub fn update_with_uris(
    activity_uri: &str,
    actor_uri: &str,
    published_at: NaiveDateTime,
    object: Value,
) -> Value {
    json!({
        "@context": note_context(),
        "id": activity_uri,
        "type": "Update",
        "actor": actor_uri,
        "published": timestamp(published_at),
        "to": object["to"],
        "cc": object["cc"],
        "object": object
    })
}

#[must_use]
pub fn delete_with_uris(
    activity_uri: &str,
    actor_uri: &str,
    object_uri: &str,
    atom_uri: &str,
) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": activity_uri,
        "type": "Delete",
        "actor": actor_uri,
        "to": [PUBLIC_ADDRESS],
        "object": {
            "id": object_uri,
            "type": "Tombstone",
            "atomUri": atom_uri
        }
    })
}

#[must_use]
pub fn delete_actor_with_uris(activity_uri: &str, actor_uri: &str) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": activity_uri,
        "type": "Delete",
        "actor": actor_uri,
        "to": [PUBLIC_ADDRESS],
        "object": actor_uri
    })
}

#[must_use]
pub fn delete_actor_delivery_logical_key(actor_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("delete-actor", actor_uri, inbox_url)
}

#[must_use]
pub fn follow_with_uris(follow_uri: &str, actor_uri: &str, target_uri: &str) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": follow_uri,
        "type": "Follow",
        "actor": actor_uri,
        "object": target_uri
    })
}

#[must_use]
pub fn undo_follow_with_uris(
    undo_uri: &str,
    actor_uri: &str,
    follow_uri: &str,
    target_uri: &str,
) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": undo_uri,
        "type": "Undo",
        "actor": actor_uri,
        "object": follow_with_uris(follow_uri, actor_uri, target_uri)
    })
}

#[must_use]
pub fn like_with_uris(activity_uri: &str, actor_uri: &str, object_uri: &str) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": activity_uri,
        "type": "Like",
        "actor": actor_uri,
        "object": object_uri
    })
}

#[must_use]
pub fn undo_like_with_uris(
    undo_uri: &str,
    actor_uri: &str,
    like_uri: &str,
    object_uri: &str,
) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": undo_uri,
        "type": "Undo",
        "actor": actor_uri,
        "object": like_with_uris(like_uri, actor_uri, object_uri)
    })
}

#[must_use]
pub fn like_delivery_logical_key(like_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("like", like_uri, inbox_url)
}

#[must_use]
pub fn undo_like_delivery_logical_key(like_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("undo-like", like_uri, inbox_url)
}

/// Addresses a locally authored Announce using its canonical local actor URI.
/// Local collection columns may be empty or stale; they are not authoritative.
pub(crate) fn local_announce_audience(
    visibility: StatusVisibility,
    actor_uri: &str,
) -> (Value, Value) {
    let followers = json!([format!("{actor_uri}/followers")]);
    match visibility {
        StatusVisibility::Public => (json!([PUBLIC_ADDRESS]), followers),
        StatusVisibility::Unlisted => (followers, json!([PUBLIC_ADDRESS])),
        StatusVisibility::Private => (followers, json!([])),
        StatusVisibility::Direct | StatusVisibility::Limited | StatusVisibility::Unknown(_) => {
            (json!([]), json!([]))
        }
    }
}

#[must_use]
pub fn announce_with_uris(
    activity_uri: &str,
    actor_uri: &str,
    published_at: NaiveDateTime,
    object_uri: &str,
    to: Value,
    cc: Value,
) -> Value {
    announce_with_object(
        activity_uri,
        actor_uri,
        published_at,
        Value::String(object_uri.to_owned()),
        to,
        cc,
    )
}

#[must_use]
pub fn announce_with_object(
    activity_uri: &str,
    actor_uri: &str,
    published_at: NaiveDateTime,
    object: Value,
    to: Value,
    cc: Value,
) -> Value {
    let context = if object.is_object() {
        note_context()
    } else {
        Value::String(ACTIVITY_STREAMS_CONTEXT.to_owned())
    };
    json!({
        "@context": context,
        "id": activity_uri,
        "type": "Announce",
        "actor": actor_uri,
        "published": timestamp(published_at),
        "to": to,
        "cc": cc,
        "object": object
    })
}

#[must_use]
pub fn undo_announce_with_uris(
    undo_uri: &str,
    actor_uri: &str,
    announce_uri: &str,
    published_at: NaiveDateTime,
    object_uri: &str,
    to: Value,
    cc: Value,
) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": undo_uri,
        "type": "Undo",
        "actor": actor_uri,
        "to": [PUBLIC_ADDRESS],
        "object": announce_with_uris(
            announce_uri,
            actor_uri,
            published_at,
            object_uri,
            to,
            cc
        )
    })
}

#[must_use]
pub fn announce_delivery_logical_key(announce_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("announce", announce_uri, inbox_url)
}

#[must_use]
pub fn undo_announce_delivery_logical_key(announce_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("undo-announce", announce_uri, inbox_url)
}

#[must_use]
pub(crate) fn status_delivery_logical_key(status_id: i64, inbox_url: &str) -> String {
    let digest = Sha256::digest(inbox_url.as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:status:{status_id}:{digest_string}")
}

#[must_use]
pub(crate) fn status_delete_delivery_logical_key(status_id: i64, inbox_url: &str) -> String {
    let digest = Sha256::digest(inbox_url.as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:status:{status_id}:delete:{digest_string}")
}

#[must_use]
pub fn follow_delivery_logical_key(follow_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("follow", follow_uri, inbox_url)
}

#[must_use]
pub fn undo_follow_delivery_logical_key(follow_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("undo-follow", follow_uri, inbox_url)
}

#[must_use]
pub fn block_with_uris(block_uri: &str, actor_uri: &str, target_uri: &str) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": block_uri,
        "type": "Block",
        "actor": actor_uri,
        "object": target_uri
    })
}

#[must_use]
pub fn undo_block_with_uris(
    undo_uri: &str,
    actor_uri: &str,
    block_uri: &str,
    target_uri: &str,
) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": undo_uri,
        "type": "Undo",
        "actor": actor_uri,
        "object": block_with_uris(block_uri, actor_uri, target_uri)
    })
}

#[must_use]
pub fn block_delivery_logical_key(block_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("block", block_uri, inbox_url)
}

#[must_use]
pub fn undo_block_delivery_logical_key(block_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key_without_id("undo-block", block_uri, inbox_url)
}

#[must_use]
pub fn accept(
    origin: &Url,
    target_account: &Account,
    follow_id: i64,
    follow_uri: &str,
    source_account: &Account,
) -> Value {
    let target_uri = actor_url(origin, target_account);
    accept_with_uris(
        &target_uri,
        follow_id,
        follow_uri,
        &actor_url(origin, source_account),
    )
}

#[must_use]
pub fn accept_with_uris(
    target_uri: &str,
    follow_id: i64,
    follow_uri: &str,
    source_uri: &str,
) -> Value {
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": format!("{target_uri}#accepts/follows/{follow_id}"),
        "type": "Accept",
        "actor": target_uri,
        "object": {
            "id": follow_uri,
            "type": "Follow",
            "actor": source_uri,
            "object": target_uri
        }
    })
}

#[must_use]
pub fn accept_delivery_logical_key(follow_id: i64, follow_uri: &str, inbox_url: &str) -> String {
    relationship_delivery_logical_key("accept", follow_id, follow_uri, inbox_url)
}

#[must_use]
pub fn reject_with_uris(
    target_uri: &str,
    follow_id: Option<i64>,
    follow_uri: &str,
    source_uri: &str,
) -> Value {
    let follow_id = follow_id.map_or_else(String::new, |id| id.to_string());
    json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": format!("{target_uri}#rejects/follows/{follow_id}"),
        "type": "Reject",
        "actor": target_uri,
        "object": {
            "id": follow_uri,
            "type": "Follow",
            "actor": source_uri,
            "object": target_uri
        }
    })
}

#[must_use]
pub fn reject_delivery_logical_key(
    target_account_id: i64,
    follow_uri: &str,
    inbox_url: &str,
) -> String {
    relationship_delivery_logical_key("reject", target_account_id, follow_uri, inbox_url)
}

fn relationship_delivery_logical_key(
    kind: &str,
    identifier: i64,
    follow_uri: &str,
    inbox_url: &str,
) -> String {
    let digest = Sha256::digest(format!("{follow_uri}\n{inbox_url}").as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:{kind}:{identifier}:{digest_string}")
}

fn relationship_delivery_logical_key_without_id(
    kind: &str,
    follow_uri: &str,
    inbox_url: &str,
) -> String {
    let digest = Sha256::digest(format!("{follow_uri}\n{inbox_url}").as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:{kind}:{digest_string}")
}

fn media_url(origin: &Url, media_root_url: &str, attachment: &MediaAttachment) -> Option<String> {
    if !attachment.remote_url.is_empty() {
        return Some(attachment.remote_url.clone());
    }
    let file_name = attachment.file_file_name.clone()?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: attachment.id,
        remote: false,
        storage_schema_version: attachment.file_storage_schema_version,
        file_name,
        content_type: attachment.file_content_type.clone(),
        variant: None,
    };
    paperclip_url(origin, media_root_url, &metadata, "original")
}

fn media_thumbnail_url(
    origin: &Url,
    media_root_url: &str,
    attachment: &MediaAttachment,
) -> Option<String> {
    let file_name = attachment.thumbnail_file_name.clone()?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaThumbnail,
        id: attachment.id,
        remote: attachment
            .thumbnail_remote_url
            .as_deref()
            .is_some_and(|url| !rails_blank(url)),
        storage_schema_version: attachment.thumbnail_storage_schema_version,
        file_name,
        content_type: attachment.thumbnail_content_type.clone(),
        variant: None,
    };
    paperclip_url(origin, media_root_url, &metadata, "original")
}

fn avatar_url(origin: &Url, media_root_url: &str, account: &Account) -> Option<String> {
    if let Some(remote_url) = account
        .avatar_remote_url
        .as_deref()
        .filter(|url| !url.is_empty())
    {
        return Some(remote_url.to_owned());
    }
    let file_name = account.avatar_file_name.clone()?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::AccountAvatar,
        id: account.id,
        remote: false,
        storage_schema_version: account.avatar_storage_schema_version,
        file_name,
        content_type: account.avatar_content_type.clone(),
        variant: None,
    };
    paperclip_url(origin, media_root_url, &metadata, "original")
}

fn header_url(origin: &Url, media_root_url: &str, account: &Account) -> Option<String> {
    if !account.header_remote_url.is_empty() {
        return Some(account.header_remote_url.clone());
    }
    let file_name = account.header_file_name.clone()?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::AccountHeader,
        id: account.id,
        remote: false,
        storage_schema_version: account.header_storage_schema_version,
        file_name,
        content_type: account.header_content_type.clone(),
        variant: None,
    };
    paperclip_url(origin, media_root_url, &metadata, "original")
}

fn paperclip_url(
    origin: &Url,
    media_root_url: &str,
    metadata: &PaperclipMetadata,
    style: &str,
) -> Option<String> {
    let relative_path = metadata.relative_path(style)?;
    let mut root = Url::parse(media_root_url)
        .or_else(|_| origin.join(media_root_url.trim_start_matches('/')))
        .ok()?;
    root.set_path(&format!("{}/", root.path().trim_end_matches('/')));
    root.join(&relative_path).ok().map(|url| url.to_string())
}

#[must_use]
pub fn ordered_collection(id: String, total: i64, first: String, last: Option<String>) -> Value {
    let mut value = json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": id,
        "type": "OrderedCollection",
        "totalItems": total,
        "first": first
    });
    if let Some(last) = last {
        value["last"] = json!(last);
    }
    value
}

#[must_use]
pub fn ordered_page(
    id: String,
    part_of: String,
    total: Option<i64>,
    items: Vec<Value>,
    next: Option<String>,
    prev: Option<String>,
) -> Value {
    let mut value = json!({
        "@context": ACTIVITY_STREAMS_CONTEXT,
        "id": id,
        "type": "OrderedCollectionPage",
        "partOf": part_of,
        "orderedItems": items
    });
    if let Some(total) = total {
        value["totalItems"] = json!(total);
    }
    if let Some(next) = next {
        value["next"] = json!(next);
    }
    if let Some(prev) = prev {
        value["prev"] = json!(prev);
    }
    value
}

#[must_use]
pub fn timestamp(value: chrono::NaiveDateTime) -> String {
    chrono::DateTime::<Utc>::from_naive_utc_and_offset(value, Utc)
        .to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use serde_json::{Value, json};

    use super::{
        CustomEmoji, PUBLIC_ADDRESS, accept, actor, actor_url, actor_with_media,
        announce_with_uris, block_with_uris, create, delete_actor_with_uris, delete_with_uris,
        follow_with_uris, host_meta, like_with_uris, note, note_context, question,
        quote_authorization, quote_authorization_url, reject_with_uris, status_activity,
        status_url, undo_announce_with_uris, undo_block_with_uris, undo_follow_with_uris,
        undo_like_with_uris, update_actor, update_with_uris, vote_with_uris,
    };
    use crate::mastodon::records::{Account, MediaAttachment, Mention, Poll, Status};
    use crate::mastodon::types::{AccountIdScheme, RawI32, RawString, StatusVisibility};

    #[test]
    fn local_announce_audiences_preserve_visibility() {
        for actor_uri in [
            "https://local.example/users/alice",
            "https://local.example/ap/users/42",
            "https://local.example/actor",
        ] {
            let followers = json!([format!("{actor_uri}/followers")]);
            for (visibility, to, cc) in [
                (
                    StatusVisibility::Public,
                    json!([PUBLIC_ADDRESS]),
                    followers.clone(),
                ),
                (
                    StatusVisibility::Unlisted,
                    followers.clone(),
                    json!([PUBLIC_ADDRESS]),
                ),
                (StatusVisibility::Private, followers, json!([])),
                (StatusVisibility::Direct, json!([]), json!([])),
                (StatusVisibility::Limited, json!([]), json!([])),
                (StatusVisibility::Unknown(99), json!([]), json!([])),
            ] {
                assert_eq!(
                    super::local_announce_audience(visibility, actor_uri),
                    (to, cc)
                );
            }
        }
    }

    fn account(id_scheme: Option<AccountIdScheme>) -> Account {
        let timestamp = DateTime::<Utc>::UNIX_EPOCH.naive_utc();
        Account {
            id: 42,
            username: "alice".to_owned(),
            domain: None,
            actor_type: Some(RawString("Person".to_owned())),
            display_name: "Alice".to_owned(),
            note: String::new(),
            uri: String::new(),
            url: None,
            also_known_as: None,
            attribution_domains: None,
            fields: None,
            avatar_content_type: None,
            avatar_description: String::new(),
            avatar_file_name: None,
            avatar_file_size: None,
            avatar_remote_url: None,
            avatar_storage_schema_version: None,
            avatar_updated_at: None,
            collections_url: None,
            discoverable: Some(true),
            feature_approval_policy: RawI32(0),
            featured_collection_url: None,
            followers_url: String::new(),
            following_url: String::new(),
            header_content_type: None,
            header_description: String::new(),
            header_file_name: None,
            header_file_size: None,
            header_remote_url: String::new(),
            header_storage_schema_version: None,
            header_updated_at: None,
            hide_collections: Some(false),
            id_scheme,
            inbox_url: String::new(),
            indexable: true,
            locked: false,
            memorial: false,
            moved_to_account_id: None,
            outbox_url: String::new(),
            protocol: RawI32(0),
            public_key: "key".to_owned(),
            private_key: None,
            sensitized_at: None,
            shared_inbox_url: String::new(),
            show_featured: true,
            show_media: true,
            show_media_replies: true,
            silenced_at: None,
            suspended_at: None,
            suspension_origin: None,
            trendable: Some(true),
            created_at: timestamp,
            updated_at: timestamp,
            has_user: true,
            login_capable_user: true,
            has_pending_user: false,
            has_unconfirmed_user: false,
        }
    }

    fn status() -> Status {
        let timestamp = DateTime::<Utc>::UNIX_EPOCH.naive_utc();
        Status {
            id: 7,
            account_id: 42,
            application_id: None,
            text: "hello".to_owned(),
            spoiler_text: String::new(),
            visibility: StatusVisibility::Public,
            local: Some(true),
            uri: None,
            url: None,
            language: Some("en".to_owned()),
            sensitive: false,
            reply: false,
            ordered_media_attachment_ids: None,
            conversation_id: None,
            in_reply_to_account_id: None,
            in_reply_to_id: None,
            reblog_of_id: None,
            poll_id: None,
            quote_approval_policy: RawI32(0),
            deleted_at: None,
            edited_at: None,
            created_at: timestamp,
            updated_at: timestamp,
        }
    }

    fn note_with_media(
        origin: &url::Url,
        account: &Account,
        status: &Status,
        media: &[MediaAttachment],
    ) -> Value {
        note(
            origin,
            "example.test",
            status,
            account,
            "/system",
            media,
            &[],
            &[],
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            0,
        )
    }

    fn basic_note(origin: &url::Url, account: &Account, status: &Status) -> Value {
        basic_note_with_quote_authorization(origin, account, status, None)
    }

    fn basic_note_with_quote_authorization(
        origin: &url::Url,
        account: &Account,
        status: &Status,
        quote_authorization: Option<&str>,
    ) -> Value {
        note(
            origin,
            "example.test",
            status,
            account,
            "/system",
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            None,
            None,
            quote_authorization,
            None,
            0,
            0,
        )
    }

    #[test]
    fn actor_urls_respect_the_account_id_scheme() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        assert_eq!(
            actor_url(&origin, &account(Some(AccountIdScheme::Username))),
            "https://example.test/users/alice"
        );
        assert_eq!(
            actor_url(&origin, &account(Some(AccountIdScheme::Numeric))),
            "https://example.test/ap/users/42"
        );
    }

    #[test]
    fn actor_media_urls_use_the_configured_paperclip_root() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let mut account = account(Some(AccountIdScheme::Username));
        account.avatar_file_name = Some("avatar.png".to_owned());
        account.avatar_content_type = Some("image/png".to_owned());
        account.avatar_storage_schema_version = Some(1);
        account.header_file_name = Some("header.jpg".to_owned());
        account.header_content_type = Some("image/jpeg".to_owned());
        account.header_storage_schema_version = Some(1);

        let relative = actor_with_media(&origin, "example.test", "/system", &account, &[], &[]);
        assert_eq!(
            relative["icon"]["url"],
            "https://example.test/system/accounts/avatars/000/000/042/original/avatar.png"
        );
        assert_eq!(
            relative["image"]["url"],
            "https://example.test/system/accounts/headers/000/000/042/original/header.jpg"
        );

        let absolute = actor_with_media(
            &origin,
            "example.test",
            "https://media.example/assets",
            &account,
            &[],
            &[],
        );
        assert_eq!(
            absolute["icon"]["url"],
            "https://media.example/assets/accounts/avatars/000/000/042/original/avatar.png"
        );
        assert_eq!(
            absolute["image"]["url"],
            "https://media.example/assets/accounts/headers/000/000/042/original/header.jpg"
        );
    }

    #[test]
    fn quote_authorization_uses_mastodon_reference_shape() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let quoted_account = account(Some(AccountIdScheme::Username));
        let mut quoting_account = account(Some(AccountIdScheme::Username));
        quoting_account.id = 43;
        quoting_account.username = "bob".to_owned();
        quoting_account.domain = Some("remote.test".to_owned());
        quoting_account.uri = "https://remote.test/users/bob".to_owned();
        let mut quoted_status = status();
        quoted_status.id = 8;
        let mut quoting_status = status();
        quoting_status.id = 9;
        quoting_status.account_id = 43;
        quoting_status.uri = Some("https://remote.test/statuses/9".to_owned());

        let value = quote_authorization(
            &origin,
            &quoted_account,
            &quoted_status,
            &quoting_account,
            &quoting_status,
            7,
        );

        assert_eq!(
            value["id"],
            "https://example.test/users/alice/quote_authorizations/7"
        );
        assert_eq!(value["type"], "QuoteAuthorization");
        assert_eq!(value["attributedTo"], "https://example.test/users/alice");
        assert_eq!(value["interactingObject"], "https://remote.test/statuses/9");
        assert_eq!(
            value["interactionTarget"],
            "https://example.test/users/alice/statuses/8"
        );
        assert_eq!(
            quote_authorization_url(
                &origin,
                &Account {
                    id_scheme: Some(AccountIdScheme::Numeric),
                    ..quoted_account
                },
                7,
            ),
            "https://example.test/ap/users/42/quote_authorizations/7"
        );
    }

    #[test]
    fn suspended_actors_mask_profile_fields() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let mut account = account(Some(AccountIdScheme::Username));
        account.suspended_at = Some(DateTime::<Utc>::UNIX_EPOCH.naive_utc());
        let value = actor(&origin, "example.test", &account);
        assert_eq!(value["name"], "alice");
        assert_eq!(value["summary"], "");
        assert_eq!(value["discoverable"], false);
        assert_eq!(value["indexable"], false);
        assert_eq!(value["manuallyApprovesFollowers"], false);
        assert_eq!(value["suspended"], true);
    }

    #[test]
    fn actor_updates_include_profile_media_and_fields() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let mut account = account(Some(AccountIdScheme::Username));
        account.updated_at =
            DateTime::<Utc>::UNIX_EPOCH.naive_utc() + chrono::Duration::seconds(42);
        account.fields = Some(json!([
            {"name": "Website", "value": "https://example.test"}
        ]));
        account.avatar_content_type = Some("image/png".to_owned());
        account.avatar_description = "Avatar".to_owned();
        account.avatar_file_name = Some("avatar.png".to_owned());
        account.avatar_storage_schema_version = Some(1);
        account.header_content_type = Some("image/jpeg".to_owned());
        account.header_description = "Header".to_owned();
        account.header_file_name = Some("header.jpg".to_owned());
        account.header_storage_schema_version = Some(1);

        let emoji = CustomEmoji {
            id: 12_001,
            shortcode: "party_blob".to_owned(),
            file_name: "party.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            storage_schema_version: None,
            updated_at: DateTime::<Utc>::UNIX_EPOCH.naive_utc(),
        };
        let value = update_actor(
            &origin,
            "example.test",
            "/system",
            &account,
            &["profiletag".to_owned()],
            &[emoji],
        );

        assert_eq!(value["type"], "Update");
        assert_eq!(
            value["id"],
            "https://example.test/users/alice#updates/42000000"
        );
        assert_eq!(value["actor"], "https://example.test/users/alice");
        assert_eq!(value["to"], json!([PUBLIC_ADDRESS]));
        assert_eq!(value["object"]["icon"]["type"], "Image");
        assert_eq!(value["object"]["icon"]["mediaType"], "image/png");
        assert_eq!(value["object"]["icon"]["summary"], "Avatar");
        assert_eq!(value["object"]["image"]["mediaType"], "image/jpeg");
        assert_eq!(value["object"]["attachment"][0]["type"], "PropertyValue");
        assert_eq!(value["object"]["attachment"][0]["name"], "Website");
        assert_eq!(value["object"]["tag"][0]["type"], "Emoji");
        assert_eq!(value["object"]["tag"][0]["name"], ":party_blob:");
        assert_eq!(value["object"]["tag"][1]["type"], "Hashtag");
        assert_eq!(value["object"]["tag"][1]["name"], "#profiletag");

        let parsed = crate::mastodon::activitypub_inbox::parse_activity(&value.to_string())
            .expect("serialized actor Update with Image.url must parse");
        assert_eq!(
            parsed,
            crate::mastodon::activitypub_inbox::InboxActivity::UpdateActor {
                actor_uri: "https://example.test/users/alice".to_owned(),
                object: value["object"].clone(),
            }
        );
    }

    #[test]
    fn note_separates_activity_and_profile_urls_and_formats_quotes() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let account = account(Some(AccountIdScheme::Username));
        let status = status();
        let value = note(
            &origin,
            "example.test",
            &status,
            &account,
            "/system",
            &[],
            &[],
            &[],
            &[],
            Some("https://remote.test/@bob/9"),
            Some("https://remote.test/status/8"),
            Some("https://remote.test/status/8#atom"),
            Some("https://example.test/conversations/7"),
            Some("https://remote.test/status/8"),
            None,
            None,
            0,
            0,
        );
        assert_eq!(value["id"], "https://example.test/users/alice/statuses/7");
        assert_eq!(value["url"], "https://example.test/@alice/7");
        assert_eq!(value["inReplyTo"], "https://remote.test/status/8");
        assert_eq!(
            value["inReplyToAtomUri"],
            "https://remote.test/status/8#atom"
        );
        assert_eq!(
            value["conversation"],
            "https://example.test/conversations/7"
        );
        assert_eq!(value["context"], "https://example.test/conversations/7");
        assert_eq!(value["quote"], "https://remote.test/status/8");
        assert_eq!(value["quoteUri"], "https://remote.test/status/8");
        assert_eq!(value["_misskey_quote"], "https://remote.test/status/8");
        assert!(value["content"].as_str().is_some_and(|content| {
            content.contains("quote-inline") && content.contains("https://remote.test/@bob/9")
        }));
        assert_eq!(
            status_url(&origin, &account, &status),
            "https://example.test/@alice/7"
        );
    }

    #[test]
    fn note_serializes_automatic_quote_interaction_policy() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let account = account(Some(AccountIdScheme::Username));
        let mut status = status();

        status.quote_approval_policy = RawI32(2 << 16);
        let value = basic_note(&origin, &account, &status);
        assert_eq!(
            value["interactionPolicy"]["canQuote"]["automaticApproval"],
            json!([PUBLIC_ADDRESS])
        );

        status.quote_approval_policy = RawI32((4 | 8) << 16);
        let value = basic_note(&origin, &account, &status);
        assert_eq!(
            value["interactionPolicy"]["canQuote"]["automaticApproval"],
            json!([
                "https://example.test/users/alice/followers",
                "https://example.test/users/alice/following"
            ])
        );

        status.quote_approval_policy = RawI32(1 << 16);
        let value = basic_note(&origin, &account, &status);
        assert_eq!(
            value["interactionPolicy"]["canQuote"]["automaticApproval"],
            json!(["https://example.test/users/alice"])
        );
    }

    #[test]
    fn note_serializes_quote_authorization_only_when_supplied() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let account = account(Some(AccountIdScheme::Username));
        let status = status();
        let value = basic_note(&origin, &account, &status);
        assert!(
            !value
                .as_object()
                .expect("note is an object")
                .contains_key("quoteAuthorization")
        );

        let value = basic_note_with_quote_authorization(
            &origin,
            &account,
            &status,
            Some("https://remote.test/quote-authorizations/7"),
        );
        assert_eq!(
            value["quoteAuthorization"],
            "https://remote.test/quote-authorizations/7"
        );
    }

    #[test]
    fn note_serializes_media_blurhash() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let account = account(Some(AccountIdScheme::Username));
        let status = status();
        let media = MediaAttachment {
            id: 9,
            account_id: Some(42),
            status_id: Some(7),
            media_type: RawI32(0),
            processing: None,
            description: Some("A remote image".to_owned()),
            remote_url: "https://media.example/image.jpg".to_owned(),
            file_content_type: Some("image/jpeg".to_owned()),
            file_file_name: None,
            file_file_size: None,
            file_meta: Some(json!({
                "original": {"width": 640, "height": 480},
                "focus": {"x": 0.25, "y": -0.5}
            })),
            file_storage_schema_version: None,
            file_updated_at: None,
            scheduled_status_id: None,
            shortcode: None,
            thumbnail_content_type: Some("image/png".to_owned()),
            thumbnail_file_name: Some("thumb.png".to_owned()),
            thumbnail_file_size: None,
            thumbnail_remote_url: None,
            thumbnail_storage_schema_version: Some(1),
            thumbnail_updated_at: None,
            blurhash: Some("L00000000000000000000000000000000".to_owned()),
            created_at: status.created_at,
            updated_at: status.updated_at,
        };
        let value = note_with_media(&origin, &account, &status, std::slice::from_ref(&media));

        assert_eq!(
            value["attachment"][0]["url"],
            "https://media.example/image.jpg"
        );
        assert_eq!(
            value["attachment"][0]["blurhash"],
            "L00000000000000000000000000000000"
        );
        assert_eq!(value["attachment"][0]["width"], 640);
        assert_eq!(value["attachment"][0]["height"], 480);
        assert_eq!(value["attachment"][0]["focalPoint"], json!([0.25, -0.5]));
        assert_eq!(
            value["attachment"][0]["icon"],
            json!({
                "type": "Image",
                "mediaType": "image/png",
                "url": "https://example.test/system/media_attachments/thumbnails/000/000/009/original/thumb.png"
            })
        );

        let mut cached_media = media.clone();
        cached_media.thumbnail_remote_url = Some("https://media.example/thumb.png".to_owned());
        let value = note_with_media(&origin, &account, &status, &[cached_media]);
        assert_eq!(
            value["attachment"][0]["icon"]["url"],
            "https://example.test/system/cache/media_attachments/thumbnails/000/000/009/original/thumb.png"
        );

        let mut malformed_media = media;
        malformed_media.file_meta = Some(json!({"focus": {"x": 0.25}}));
        malformed_media.thumbnail_file_name = None;
        malformed_media.thumbnail_remote_url = Some("https://media.example/thumb.png".to_owned());
        let value = note_with_media(&origin, &account, &status, &[malformed_media]);
        assert!(
            !value["attachment"][0]
                .as_object()
                .expect("attachment is an object")
                .contains_key("focalPoint")
        );
        assert!(
            !value["attachment"][0]
                .as_object()
                .expect("attachment is an object")
                .contains_key("width")
        );
        assert!(
            !value["attachment"][0]
                .as_object()
                .expect("attachment is an object")
                .contains_key("height")
        );
        assert!(
            !value["attachment"][0]
                .as_object()
                .expect("attachment is an object")
                .contains_key("icon")
        );
    }

    #[test]
    fn note_marks_sensitized_accounts_sensitive() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let mut account = account(Some(AccountIdScheme::Username));
        account.sensitized_at = Some(DateTime::<Utc>::UNIX_EPOCH.naive_utc());
        let status = status();
        let value = note(
            &origin,
            "example.test",
            &status,
            &account,
            "/system",
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            0,
        );

        assert_eq!(value["sensitive"], true);
    }

    #[test]
    fn serialized_notes_round_trip_through_inbox_with_and_without_content_warnings() {
        use crate::mastodon::activitypub_inbox::{InboxActivity, parse_activity};

        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let account = account(Some(AccountIdScheme::Username));
        for spoiler_text in ["", "Content warning"] {
            let mut status = status();
            status.spoiler_text = spoiler_text.to_owned();
            let object = basic_note(&origin, &account, &status);
            assert_eq!(object["summary"].is_null(), spoiler_text.is_empty());
            let create = create(&origin, &account, &status, object.clone());
            assert!(matches!(
                parse_activity(&create.to_string()).expect("serialized Create must parse"),
                InboxActivity::CreateNote { object: parsed, .. } if parsed == object
            ));

            status.edited_at = Some(status.updated_at + chrono::Duration::seconds(1));
            let object = basic_note(&origin, &account, &status);
            let update = update_with_uris(
                "https://example.test/users/alice/statuses/7#updates/1",
                "https://example.test/users/alice",
                status.edited_at.expect("edit timestamp"),
                object.clone(),
            );
            assert!(matches!(
                parse_activity(&update.to_string()).expect("serialized Update must parse"),
                InboxActivity::UpdateNote { object: parsed, .. } if parsed == object
            ));
        }
    }

    #[test]
    fn create_activity_wraps_a_note_with_the_local_actor_and_activity_id() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let account = account(Some(AccountIdScheme::Username));
        let status = status();
        let object = note(
            &origin,
            "example.test",
            &status,
            &account,
            "/system",
            &[],
            &[],
            &[],
            &[],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            0,
        );

        let value = create(&origin, &account, &status, object.clone());

        assert_eq!(value["type"], "Create");
        assert_eq!(
            value["id"],
            "https://example.test/users/alice/statuses/7/activity"
        );
        assert_eq!(value["actor"], "https://example.test/users/alice");
        assert_eq!(value["published"], "1970-01-01T00:00:00Z");
        assert_eq!(value["to"], object["to"]);
        assert_eq!(value["cc"], object["cc"]);
        assert_eq!(value["object"], object);
    }

    #[test]
    fn note_appends_local_emoji_tags_without_losing_mentions_or_hashtags() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let account = account(Some(AccountIdScheme::Username));
        let status = status();
        let emoji = CustomEmoji {
            id: 12_001,
            shortcode: "party_blob".to_owned(),
            file_name: "party.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            storage_schema_version: None,
            updated_at: DateTime::<Utc>::UNIX_EPOCH.naive_utc(),
        };
        let mention = Mention {
            id: 1,
            account_id: account.id,
            status_id: status.id,
            silent: false,
        };
        let value = note(
            &origin,
            "example.test",
            &status,
            &account,
            "https://media.example/system",
            &[],
            &[(mention, account.clone())],
            &[("rust".to_owned(), "Rust".to_owned())],
            &[emoji],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            0,
        );

        assert_eq!(value["tag"][0]["type"], "Mention");
        assert_eq!(value["tag"][1]["type"], "Hashtag");
        assert_eq!(value["tag"][2]["type"], "Emoji");
        assert_eq!(value["tag"][2]["name"], ":party_blob:");
        assert_eq!(value["tag"][2]["id"], "https://example.test/emojis/12001");
        assert_eq!(
            value["tag"][2]["icon"]["url"],
            "https://media.example/system/custom_emojis/images/000/012/001/original/party.png"
        );
    }

    #[test]
    fn questions_and_votes_match_mastodon_poll_shapes() {
        let expires_at = DateTime::<Utc>::UNIX_EPOCH.naive_utc() + chrono::Duration::hours(1);
        let poll = Poll {
            id: 9,
            account_id: 42,
            status_id: 7,
            options: vec!["Tea".to_owned(), "Coffee".to_owned()],
            cached_tallies: vec![2, 1],
            votes_count: 3,
            voters_count: Some(3),
            multiple: false,
            hide_totals: true,
            expires_at: Some(expires_at),
        };
        let active = question(
            json!({"id": "https://example.test/users/alice/statuses/7", "type": "Note"}),
            &poll,
            DateTime::<Utc>::UNIX_EPOCH.naive_utc(),
        );
        assert_eq!(active["type"], "Question");
        assert!(active.get("anyOf").is_none());
        assert_eq!(active["oneOf"][0]["name"], "Tea");
        assert_eq!(active["oneOf"][0]["replies"]["totalItems"], Value::Null);
        assert!(active.get("closed").is_none());
        assert_eq!(active["votersCount"], 3);
        assert_eq!(note_context()[1]["Emoji"], "toot:Emoji");
        assert_eq!(note_context()[1]["votersCount"], "toot:votersCount");

        let closed = question(active, &poll, expires_at);
        assert_eq!(closed["closed"], "1970-01-01T01:00:00Z");
        assert_eq!(closed["oneOf"][0]["replies"]["totalItems"], 2);

        let vote = vote_with_uris(
            "https://remote.test/users/bob#votes/1",
            "https://remote.test/users/bob",
            "https://example.test/users/alice/statuses/7",
            "https://example.test/users/alice",
            "Tea",
        );
        assert_eq!(vote["type"], "Create");
        assert_eq!(vote["object"]["type"], "Note");
        assert_eq!(vote["object"]["name"], "Tea");
        assert_eq!(
            vote["object"]["inReplyTo"],
            "https://example.test/users/alice/statuses/7"
        );
    }

    #[test]
    fn status_activity_selects_create_or_announce_from_the_status_shape() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let account = account(Some(AccountIdScheme::Username));
        let status = status();
        let object = json!({"id": "https://example.test/users/alice/statuses/7"});

        let create = status_activity(
            &origin,
            &account,
            &status,
            object.clone(),
            json!([PUBLIC_ADDRESS]),
            json!([]),
        );
        assert_eq!(create["type"], "Create");
        assert_eq!(create["object"], object);

        let mut boost = status;
        boost.id = 8;
        boost.reblog_of_id = Some(7);
        let announce = status_activity(
            &origin,
            &account,
            &boost,
            json!("https://example.test/users/bob/statuses/7"),
            json!([PUBLIC_ADDRESS]),
            json!([]),
        );
        assert_eq!(announce["type"], "Announce");
        assert_eq!(
            announce["id"],
            "https://example.test/users/alice/statuses/8/activity"
        );
        assert_eq!(
            announce["object"],
            "https://example.test/users/bob/statuses/7"
        );
    }

    #[test]
    fn update_and_delete_activities_preserve_status_identity_and_audience() {
        let actor_uri = "https://example.test/users/alice";
        let object_uri = "https://example.test/users/alice/statuses/7";
        let object = json!({
            "id": object_uri,
            "type": "Note",
            "to": [PUBLIC_ADDRESS],
            "cc": ["https://example.test/users/alice/followers"]
        });
        let edited_at = DateTime::<Utc>::UNIX_EPOCH.naive_utc() + chrono::Duration::seconds(42);
        let update = update_with_uris(
            &format!("{object_uri}#updates/42"),
            actor_uri,
            edited_at,
            object.clone(),
        );
        assert_eq!(update["type"], "Update");
        assert_eq!(
            update["id"],
            "https://example.test/users/alice/statuses/7#updates/42"
        );
        assert_eq!(update["published"], "1970-01-01T00:00:42Z");
        assert_eq!(update["@context"], note_context());
        assert_eq!(
            update["@context"][1]["votersCount"], "toot:votersCount",
            "poll Updates need Mastodon's full Note/Question extension context",
        );
        assert_eq!(update["@context"][1]["Emoji"], "toot:Emoji");
        assert_eq!(update["to"], object["to"]);
        assert_eq!(update["cc"], object["cc"]);
        assert_eq!(update["object"], object);

        let delete = delete_with_uris(
            &format!("{object_uri}#delete"),
            actor_uri,
            object_uri,
            "tag:example.test,1970-01-01:objectId=7;objectType=Status",
        );
        assert_eq!(delete["type"], "Delete");
        assert_eq!(
            delete["id"],
            "https://example.test/users/alice/statuses/7#delete"
        );
        assert_eq!(delete["actor"], actor_uri);
        assert_eq!(delete["to"][0], PUBLIC_ADDRESS);
        assert_eq!(delete["object"]["type"], "Tombstone");
        assert_eq!(delete["object"]["id"], object_uri);
        assert_eq!(
            delete["object"]["atomUri"],
            "tag:example.test,1970-01-01:objectId=7;objectType=Status"
        );
        let parsed = crate::mastodon::activitypub_inbox::parse_activity(&delete.to_string())
            .expect("serialized tag-bearing Tombstone must parse");
        assert!(
            matches!(
                parsed,
                crate::mastodon::activitypub_inbox::InboxActivity::DeleteNote {
                    atom_uri: None,
                    ..
                }
            ),
            "opaque tag metadata is not an authenticated lookup alias"
        );
    }

    #[test]
    fn actor_delete_uses_the_actor_as_its_object() {
        let actor_uri = "https://example.test/users/alice";
        let value = delete_actor_with_uris(&format!("{actor_uri}#delete"), actor_uri);

        assert_eq!(value["type"], "Delete");
        assert_eq!(value["id"], "https://example.test/users/alice#delete");
        assert_eq!(value["actor"], actor_uri);
        assert_eq!(value["to"], json!([PUBLIC_ADDRESS]));
        assert_eq!(value["object"], actor_uri);
    }

    #[test]
    fn accept_activity_wraps_the_remote_follow_and_uses_the_local_actor() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let local = account(Some(AccountIdScheme::Username));
        let mut remote = account(Some(AccountIdScheme::Username));
        remote.domain = Some("remote.test".to_owned());
        remote.uri = "https://remote.test/users/bob".to_owned();

        let value = accept(
            &origin,
            &local,
            9,
            "https://remote.test/activities/follow-9",
            &remote,
        );

        assert_eq!(value["type"], "Accept");
        assert_eq!(
            value["id"],
            "https://example.test/users/alice#accepts/follows/9"
        );
        assert_eq!(value["actor"], "https://example.test/users/alice");
        assert_eq!(value["object"]["type"], "Follow");
        assert_eq!(value["object"]["actor"], "https://remote.test/users/bob");
        assert_eq!(
            value["object"]["object"],
            "https://example.test/users/alice"
        );
    }

    #[test]
    fn reject_activity_uses_the_mastodon_follow_row_identity() {
        let target_uri = "https://example.test/users/alice";
        let follow_uri = "https://remote.test/activities/follow-9";
        let source_uri = "https://remote.test/users/bob";

        let value = reject_with_uris(target_uri, Some(42), follow_uri, source_uri);
        assert_eq!(value["type"], "Reject");
        assert_eq!(
            value["id"],
            "https://example.test/users/alice#rejects/follows/42"
        );
        assert_eq!(value["actor"], target_uri);
        assert_eq!(value["object"]["id"], follow_uri);
        assert_eq!(value["object"]["actor"], source_uri);

        let immediate = reject_with_uris(target_uri, None, follow_uri, source_uri);
        assert_eq!(
            immediate["id"],
            "https://example.test/users/alice#rejects/follows/"
        );
    }

    #[test]
    fn follow_and_undo_activities_preserve_the_stable_follow_uri() {
        let follow_uri = "https://example.test/payloads/follow-42-7-9";
        let follow = follow_with_uris(
            follow_uri,
            "https://example.test/users/alice",
            "https://remote.test/users/bob",
        );
        assert_eq!(follow["id"], follow_uri);
        assert_eq!(follow["type"], "Follow");
        assert_eq!(follow["actor"], "https://example.test/users/alice");
        assert_eq!(follow["object"], "https://remote.test/users/bob");

        let undo = undo_follow_with_uris(
            "https://example.test/payloads/undo-follow-42-7-9",
            "https://example.test/users/alice",
            follow_uri,
            "https://remote.test/users/bob",
        );
        assert_eq!(undo["type"], "Undo");
        assert_eq!(undo["actor"], "https://example.test/users/alice");
        assert_eq!(undo["object"]["id"], follow_uri);
        assert_eq!(undo["object"]["type"], "Follow");
    }

    #[test]
    fn like_and_announce_activities_preserve_nested_objects_and_audiences() {
        let actor_uri = "https://example.test/users/alice";
        let object_uri = "https://remote.test/users/bob/statuses/9";
        let like_uri = "https://example.test/users/alice#likes/7";
        let like = like_with_uris(like_uri, actor_uri, object_uri);
        assert_eq!(like["type"], "Like");
        assert_eq!(like["actor"], actor_uri);
        assert_eq!(like["object"], object_uri);

        let undo_like = undo_like_with_uris(
            "https://example.test/users/alice#likes/7/undo",
            actor_uri,
            like_uri,
            object_uri,
        );
        assert_eq!(undo_like["type"], "Undo");
        assert_eq!(undo_like["object"], like);

        let announce = announce_with_uris(
            "https://example.test/users/alice/statuses/8/activity",
            actor_uri,
            DateTime::<Utc>::UNIX_EPOCH.naive_utc(),
            object_uri,
            serde_json::json!([PUBLIC_ADDRESS]),
            serde_json::json!(["https://example.test/users/alice/followers"]),
        );
        assert_eq!(announce["type"], "Announce");
        assert_eq!(announce["published"], "1970-01-01T00:00:00Z");
        assert_eq!(announce["to"][0], PUBLIC_ADDRESS);
        assert_eq!(
            announce["cc"][0],
            "https://example.test/users/alice/followers"
        );

        let undo_announce = undo_announce_with_uris(
            "https://example.test/users/alice#announces/8/undo",
            actor_uri,
            announce["id"].as_str().expect("announce has an ID"),
            DateTime::<Utc>::UNIX_EPOCH.naive_utc(),
            object_uri,
            serde_json::json!([PUBLIC_ADDRESS]),
            serde_json::json!(["https://example.test/users/alice/followers"]),
        );
        assert_eq!(undo_announce["type"], "Undo");
        assert_eq!(undo_announce["object"]["type"], "Announce");
        assert_eq!(undo_announce["object"]["object"], object_uri);
    }

    #[test]
    fn block_and_undo_activities_preserve_the_stable_block_uri() {
        let block_uri = "https://example.test/payloads/block-42-7-9";
        let block = block_with_uris(
            block_uri,
            "https://example.test/users/alice",
            "https://remote.test/users/bob",
        );
        assert_eq!(block["id"], block_uri);
        assert_eq!(block["type"], "Block");
        assert_eq!(block["actor"], "https://example.test/users/alice");
        assert_eq!(block["object"], "https://remote.test/users/bob");

        let undo = undo_block_with_uris(
            "https://example.test/payloads/undo-block-42-7-9",
            "https://example.test/users/alice",
            block_uri,
            "https://remote.test/users/bob",
        );
        assert_eq!(undo["type"], "Undo");
        assert_eq!(undo["actor"], "https://example.test/users/alice");
        assert_eq!(undo["object"]["id"], block_uri);
        assert_eq!(undo["object"]["type"], "Block");
    }

    #[test]
    fn host_meta_supports_both_wire_formats() {
        let origin = url::Url::parse("https://example.test/").expect("valid origin");
        let (json_type, json_body) = host_meta(&origin, true);
        assert_eq!(json_type, "application/json; charset=utf-8");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&json_body).expect("valid host-meta JSON")
                ["links"][0]["rel"],
            "lrdd"
        );
        let (xml_type, xml_body) = host_meta(&origin, false);
        assert_eq!(xml_type, "application/xrd+xml; charset=utf-8");
        assert!(
            String::from_utf8(xml_body)
                .expect("host-meta XML is UTF-8")
                .contains("<XRD")
        );
    }
}
