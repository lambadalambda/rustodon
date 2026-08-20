#![allow(clippy::missing_panics_doc, clippy::needless_pass_by_value)]

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use url::Url;

use crate::paperclip::{PaperclipAttachment, PaperclipMetadata};

use super::records::{Account, MediaAttachment, Mention, Status};
use super::rest::{HtmlFormatter, MentionTarget};
use super::types::AccountIdScheme;

pub const ACTIVITY_JSON: &str = "application/activity+json; charset=utf-8";
pub const JRD_JSON: &str = "application/jrd+json; charset=utf-8";
pub const XRD_XML: &str = "application/xrd+xml; charset=utf-8";
pub const ACTIVITY_STREAMS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";
pub const SECURITY_CONTEXT: &str = "https://w3id.org/security/v1";
pub const WEBFINGER_CONTEXT: &str = "https://purl.archive.org/socialweb/webfinger";
pub const PUBLIC_ADDRESS: &str = "https://www.w3.org/ns/activitystreams#Public";

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
    format!("{}/replies", status_uri(origin, account, status))
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
    quoted_url: Option<&str>,
    in_reply_to_url: Option<&str>,
    quote_identifier: Option<&str>,
    replies: Option<Value>,
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
    let mention_addresses = mentions
        .iter()
        .map(|(_, target)| Value::String(actor_url(origin, target)))
        .collect::<Vec<_>>();
    let (to, cc) = match status.visibility {
        super::StatusVisibility::Public => (
            vec![Value::String(PUBLIC_ADDRESS.to_owned())],
            std::iter::once(Value::String(followers))
                .chain(mention_addresses.clone())
                .collect(),
        ),
        super::StatusVisibility::Unlisted => (
            vec![Value::String(followers)],
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
        Some(json!({
            "type": "Document",
            "mediaType": attachment.file_content_type.clone().unwrap_or_else(|| "application/octet-stream".to_owned()),
            "url": media_url,
            "name": attachment.description
        }))
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
        "context": Value::Null,
        "published": timestamp(status.created_at),
        "url": url,
        "attributedTo": actor_url(origin, account),
        "to": to,
        "cc": cc,
        "sensitive": status.sensitive,
        "atomUri": atom_uri,
        "content": content,
        "attachment": attachments.collect::<Vec<_>>(),
        "tag": mention_tags.chain(hashtag_tags).collect::<Vec<_>>()
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
    }
    if let Some(replies) = replies {
        value["replies"] = replies;
    }
    value
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

    use super::{actor, actor_url, host_meta, note, status_url};
    use crate::mastodon::records::{Account, Status};
    use crate::mastodon::types::{AccountIdScheme, RawI32, RawString, StatusVisibility};

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
            Some("https://remote.test/@bob/9"),
            None,
            None,
            None,
        );
        assert_eq!(value["id"], "https://example.test/users/alice/statuses/7");
        assert_eq!(value["url"], "https://example.test/@alice/7");
        assert!(value["content"].as_str().is_some_and(|content| {
            content.contains("quote-inline") && content.contains("https://remote.test/@bob/9")
        }));
        assert_eq!(
            status_url(&origin, &account, &status),
            "https://example.test/@alice/7"
        );
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
