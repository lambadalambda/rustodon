use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use url::Url;

const MAX_REMOTE_EMOJIS: usize = 100;
const MAX_REMOTE_TAGS: usize = 1_000;
const MAX_REMOTE_EMOJI_SHORTCODE: usize = 2_048;
const MAX_REMOTE_EMOJI_URL: usize = 2_048;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteEmojiTag {
    pub(crate) shortcode: String,
    pub(crate) uri: Option<String>,
    pub(crate) image_url: String,
    pub(crate) media_type: Option<String>,
    pub(crate) updated_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InboxJob {
    pub(crate) body: String,
    pub(crate) delivery_target_account_id: Option<i64>,
    pub(crate) signature_key_id: String,
    pub(crate) remote_domain: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InboxActivity {
    CreateNote {
        activity_uri: String,
        actor_uri: String,
        object: Value,
        activity: Value,
    },
    CreateNoteReference {
        activity_uri: String,
        actor_uri: String,
        object_uri: String,
        to: Vec<String>,
        cc: Vec<String>,
        activity: Value,
    },
    UpdateNote {
        actor_uri: String,
        object: Value,
        activity: Value,
    },
    DeleteNote {
        actor_uri: String,
        object_uri: String,
        atom_uri: Option<String>,
        activity: Value,
    },
    Like {
        activity_uri: String,
        actor_uri: String,
        object_uri: String,
    },
    Announce {
        activity_uri: String,
        actor_uri: String,
        object_uri: String,
        embedded_note: Option<Value>,
        to: Vec<String>,
        cc: Vec<String>,
        published_at: Option<String>,
    },
    UndoLike {
        actor_uri: String,
        activity_uri: String,
        object_uri: String,
    },
    UndoAnnounce {
        actor_uri: String,
        activity_uri: String,
        object_uri: String,
    },
    Follow {
        activity_uri: String,
        actor_uri: String,
        object_uri: String,
    },
    Flag {
        activity_uri: Option<String>,
        actor_uri: String,
        object_uris: Vec<String>,
        comment: String,
    },
    Block {
        activity_uri: String,
        actor_uri: String,
        object_uri: String,
    },
    UpdateActor {
        actor_uri: String,
        object: Value,
    },
    DeleteActor {
        actor_uri: String,
        object_uri: String,
    },
    Accept {
        actor_uri: String,
        follow_uri: String,
        target_uri: Option<String>,
        nested_actor_uri: Option<String>,
    },
    Reject {
        actor_uri: String,
        follow_uri: String,
        target_uri: Option<String>,
        nested_actor_uri: Option<String>,
    },
    UndoFollow {
        actor_uri: String,
        follow_uri: String,
        target_uri: Option<String>,
        nested_actor_uri: Option<String>,
    },
    UndoReference {
        actor_uri: String,
        object_uri: String,
    },
    UndoBlock {
        actor_uri: String,
        block_uri: String,
        target_uri: Option<String>,
        nested_actor_uri: Option<String>,
    },
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InboxParseError {
    Arguments,
    Json,
    Activity,
}

impl std::fmt::Display for InboxParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Arguments => "ActivityPub inbox job arguments are invalid",
            Self::Json => "ActivityPub inbox body is invalid JSON",
            Self::Activity => "ActivityPub inbox activity is invalid",
        })
    }
}

impl std::error::Error for InboxParseError {}

pub(crate) fn parse_job_arguments(arguments: &Value) -> Result<InboxJob, InboxParseError> {
    let body = arguments
        .get("body")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(InboxParseError::Arguments)?;
    let delivery_target_account_id = match arguments.get("delivery_target_account_id") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_i64().ok_or(InboxParseError::Arguments)?),
    };
    let signature_key_id = required_text(arguments.get("signature_key_id"))?;
    let remote_domain = required_text(arguments.get("remote_domain"))?;
    Ok(InboxJob {
        body: body.to_owned(),
        delivery_target_account_id,
        signature_key_id,
        remote_domain,
    })
}

pub(crate) fn parse_activity(body: &str) -> Result<InboxActivity, InboxParseError> {
    let value = serde_json::from_str::<Value>(body).map_err(|_| InboxParseError::Json)?;
    let Value::Object(activity) = value else {
        return Err(InboxParseError::Activity);
    };
    let Some(kind) = activity.get("type").and_then(Value::as_str) else {
        return Err(InboxParseError::Activity);
    };
    match kind {
        "Create" => parse_note_create(&activity),
        "Update" => parse_update(&activity),
        "Delete" => parse_delete(&activity),
        "Like" => parse_interaction(&activity, true),
        "Announce" => parse_interaction(&activity, false),
        "Follow" => Ok(InboxActivity::Follow {
            activity_uri: required_uri(activity.get("id"))?,
            actor_uri: required_uri(activity.get("actor"))?,
            object_uri: required_uri(activity.get("object"))?,
        }),
        "Flag" => parse_flag(&activity),
        "Block" => Ok(InboxActivity::Block {
            activity_uri: required_uri(activity.get("id"))?,
            actor_uri: required_uri(activity.get("actor"))?,
            object_uri: required_uri(activity.get("object"))?,
        }),
        "Accept" => parse_follow_decision(&activity, true),
        "Reject" => parse_follow_decision(&activity, false),
        "Undo" => parse_undo(&activity),
        _ => Ok(InboxActivity::Unsupported),
    }
}

fn parse_note_create(
    activity: &serde_json::Map<String, Value>,
) -> Result<InboxActivity, InboxParseError> {
    let activity_uri = required_uri(activity.get("id"))?;
    let actor_uri = required_uri(activity.get("actor"))?;
    let Some(object) = activity.get("object") else {
        return Err(InboxParseError::Activity);
    };
    if object.is_string() {
        return Ok(InboxActivity::CreateNoteReference {
            activity_uri,
            actor_uri,
            object_uri: required_uri(Some(object))?,
            to: parse_interaction_audience(activity.get("to"))?,
            cc: parse_interaction_audience(activity.get("cc"))?,
            activity: Value::Object(activity.clone()),
        });
    }
    let Value::Object(object) = object else {
        return Err(InboxParseError::Activity);
    };
    if object.get("type").and_then(Value::as_str) != Some("Note") {
        return Ok(InboxActivity::Unsupported);
    }
    validate_note_object(&actor_uri, object)?;
    Ok(InboxActivity::CreateNote {
        activity_uri,
        actor_uri,
        object: Value::Object(object.clone()),
        activity: Value::Object(activity.clone()),
    })
}

fn parse_update(
    activity: &serde_json::Map<String, Value>,
) -> Result<InboxActivity, InboxParseError> {
    let actor_uri = required_uri(activity.get("actor"))?;
    let Some(Value::Object(object)) = activity.get("object") else {
        return Ok(InboxActivity::Unsupported);
    };
    match object.get("type").and_then(Value::as_str) {
        Some("Note") => {
            validate_note_object(&actor_uri, object)?;
            Ok(InboxActivity::UpdateNote {
                actor_uri,
                object: Value::Object(object.clone()),
                activity: Value::Object(activity.clone()),
            })
        }
        Some("Application" | "Group" | "Organization" | "Person" | "Service") => {
            parse_actor_update_parts(actor_uri, object)
        }
        Some(_) => Ok(InboxActivity::Unsupported),
        None => Err(InboxParseError::Activity),
    }
}

fn parse_delete(
    activity: &serde_json::Map<String, Value>,
) -> Result<InboxActivity, InboxParseError> {
    let actor_uri = required_uri(activity.get("actor"))?;
    let object = activity.get("object").ok_or(InboxParseError::Activity)?;
    let (object_uri, atom_uri) = match object {
        Value::String(_) => (required_uri(Some(object))?, None),
        Value::Object(object) => (
            required_uri(object.get("id"))?,
            object
                .get("atomUri")
                .map(|value| required_uri(Some(value)))
                .transpose()?,
        ),
        _ => return Err(InboxParseError::Activity),
    };
    if object_uri == actor_uri {
        return Ok(InboxActivity::DeleteActor {
            actor_uri,
            object_uri,
        });
    }
    Ok(InboxActivity::DeleteNote {
        actor_uri,
        object_uri,
        atom_uri,
        activity: Value::Object(activity.clone()),
    })
}

fn parse_interaction(
    activity: &serde_json::Map<String, Value>,
    like: bool,
) -> Result<InboxActivity, InboxParseError> {
    let activity_uri = required_uri(activity.get("id"))?;
    let actor_uri = required_uri(activity.get("actor"))?;
    let object = activity.get("object");
    let object_uri = required_uri(object)?;
    Ok(if like {
        InboxActivity::Like {
            activity_uri,
            actor_uri,
            object_uri,
        }
    } else {
        let embedded_note = match object {
            Some(Value::Object(object))
                if object.get("type").and_then(Value::as_str) == Some("Note")
                    && validate_note_object(&actor_uri, object).is_ok() =>
            {
                Some(Value::Object(object.clone()))
            }
            _ => None,
        };
        InboxActivity::Announce {
            activity_uri,
            actor_uri,
            object_uri,
            embedded_note,
            to: parse_interaction_audience(activity.get("to"))?,
            cc: parse_interaction_audience(activity.get("cc"))?,
            published_at: activity
                .get("published")
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or(InboxParseError::Activity)
                })
                .transpose()?,
        }
    })
}

fn parse_flag(activity: &serde_json::Map<String, Value>) -> Result<InboxActivity, InboxParseError> {
    let actor_uri = required_uri(activity.get("actor"))?;
    let activity_uri = activity
        .get("id")
        .filter(|value| !value.is_null())
        .and_then(|value| value_or_id(Some(value)));
    let object = activity.get("object").ok_or(InboxParseError::Activity)?;
    let values: Vec<&Value> = match object {
        Value::Array(values) => values.iter().collect(),
        _ => vec![object],
    };
    if values.len() > 100 {
        return Err(InboxParseError::Activity);
    }
    let object_uris = values
        .iter()
        .filter_map(|value| value_or_id(Some(value)))
        .collect();
    let comment = match activity.get("content") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.chars().take(5_000).collect(),
        Some(_) => return Err(InboxParseError::Activity),
    };
    Ok(InboxActivity::Flag {
        activity_uri,
        actor_uri,
        object_uris,
        comment,
    })
}

fn parse_interaction_audience(value: Option<&Value>) -> Result<Vec<String>, InboxParseError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let values: Vec<&Value> = value
        .as_array()
        .map_or_else(|| vec![value], |values| values.iter().collect());
    if values.len() > 100 {
        return Err(InboxParseError::Activity);
    }
    values
        .iter()
        .map(|value| audience_uri(Some(value)))
        .collect()
}

fn parse_actor_update_parts(
    actor_uri: String,
    object: &serde_json::Map<String, Value>,
) -> Result<InboxActivity, InboxParseError> {
    let object_uri = required_uri(object.get("id"))?;
    if object_uri != actor_uri {
        return Err(InboxParseError::Activity);
    }
    bounded_text(object, "preferredUsername", 2048)?;
    bounded_text(object, "name", 2048)?;
    bounded_text(object, "summary", 20 * 1024)?;
    for field in ["url", "inbox", "outbox", "followers", "following"] {
        optional_uri(object.get(field))?;
    }
    if let Some(endpoints) = object.get("endpoints") {
        if let Value::Object(endpoints) = endpoints {
            optional_uri(endpoints.get("sharedInbox"))?;
        } else {
            return Err(InboxParseError::Activity);
        }
    }
    for field in ["icon", "image"] {
        if let Some(value) = object.get(field) {
            optional_uri(Some(value))?;
        }
    }
    if let Some(value) = object.get("suspended")
        && !value.is_null()
        && !value.is_boolean()
    {
        return Err(InboxParseError::Activity);
    }
    Ok(InboxActivity::UpdateActor {
        actor_uri,
        object: Value::Object(object.clone()),
    })
}

pub(crate) fn validate_note_object(
    actor_uri: &str,
    object: &serde_json::Map<String, Value>,
) -> Result<(), InboxParseError> {
    let object_uri = required_uri(object.get("id"))?;
    let attributed_to = first_uri(object.get("attributedTo"))?;
    if attributed_to != actor_uri {
        return Err(InboxParseError::Activity);
    }
    let content = object
        .get("content")
        .or_else(|| {
            object
                .get("contentMap")
                .and_then(Value::as_object)
                .and_then(|map| map.values().next())
        })
        .and_then(Value::as_str)
        .ok_or(InboxParseError::Activity)?;
    if content.chars().count() > 20 * 1024 {
        return Err(InboxParseError::Activity);
    }
    for field in ["url", "inReplyTo"] {
        optional_uri(object.get(field))?;
    }
    optional_conversation_uri(object.get("conversation"))?;
    for field in ["published", "updated"] {
        if let Some(value) = object.get(field)
            && value.as_str().is_none()
        {
            return Err(InboxParseError::Activity);
        }
    }
    if let Some(value) = object.get("summary")
        && value
            .as_str()
            .is_none_or(|summary| summary.chars().count() > 20 * 1024)
    {
        return Err(InboxParseError::Activity);
    }
    if let Some(value) = object.get("sensitive")
        && value.as_bool().is_none()
    {
        return Err(InboxParseError::Activity);
    }
    for field in ["tag", "attachment"] {
        if let Some(value) = object.get(field) {
            match value {
                Value::Array(_) | Value::Object(_) | Value::String(_) | Value::Null => {}
                _ => return Err(InboxParseError::Activity),
            }
        }
    }
    for field in ["to", "cc"] {
        if let Some(value) = object.get(field)
            && !value.is_null()
        {
            match value {
                Value::Array(values) => {
                    for value in values {
                        audience_uri(Some(value))?;
                    }
                }
                Value::String(_) | Value::Object(_) => {
                    audience_uri(Some(value))?;
                }
                _ => return Err(InboxParseError::Activity),
            }
        }
    }
    for (field, limit) in [
        ("to", 100_usize),
        ("cc", 100),
        ("tag", MAX_REMOTE_TAGS),
        ("attachment", 16),
    ] {
        if object
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|values| values.len() > limit)
        {
            return Err(InboxParseError::Activity);
        }
    }
    let _ = object_uri;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) fn parse_note_emojis(object: &Value, actor_uri: &str) -> Vec<RemoteEmojiTag> {
    let Some(actor_host) = Url::parse(actor_uri)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    else {
        return Vec::new();
    };
    let Some(tags) = object.get("tag") else {
        return Vec::new();
    };
    let tags: Vec<&Value> = tags
        .as_array()
        .map_or_else(|| vec![tags], |tags| tags.iter().collect());
    let mut emojis = Vec::new();
    for tag in tags.into_iter().take(MAX_REMOTE_TAGS) {
        let Some(tag) = tag
            .as_object()
            .filter(|tag| value_includes_text(tag.get("type"), "Emoji"))
        else {
            continue;
        };
        let Some(shortcode) = tag
            .get("name")
            .and_then(Value::as_str)
            .map(|name| name.trim_matches(':'))
            .filter(|name| {
                (2..=MAX_REMOTE_EMOJI_SHORTCODE).contains(&name.len())
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        else {
            continue;
        };
        let uri = match tag.get("id").filter(|value| !value.is_null()) {
            Some(Value::String(uri)) if uri.len() <= MAX_REMOTE_EMOJI_URL => {
                let Some(url) = Url::parse(uri).ok().filter(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && url
                            .host_str()
                            .is_some_and(|host| host.eq_ignore_ascii_case(&actor_host))
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.fragment().is_none()
                }) else {
                    continue;
                };
                Some(url.to_string())
            }
            None => None,
            Some(_) => continue,
        };
        let Some(icon) = tag.get("icon").and_then(Value::as_object) else {
            continue;
        };
        let Some(image_url) = icon
            .get("url")
            .and_then(Value::as_str)
            .filter(|url| url.len() <= MAX_REMOTE_EMOJI_URL)
            .and_then(|url| Url::parse(url).ok())
            .filter(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none()
            })
        else {
            continue;
        };
        let media_type = match icon.get("mediaType").filter(|value| !value.is_null()) {
            Some(Value::String(value)) => {
                let value = value.to_ascii_lowercase();
                if !matches!(value.as_str(), "image/png" | "image/gif" | "image/webp") {
                    continue;
                }
                Some(value)
            }
            None => None,
            Some(_) => continue,
        };
        let updated_at = match tag.get("updated").filter(|value| !value.is_null()) {
            Some(Value::String(value)) => {
                let Ok(updated) = DateTime::parse_from_rfc3339(value) else {
                    continue;
                };
                let updated = updated.naive_utc();
                if updated > Utc::now().naive_utc() + chrono::Duration::hours(24) {
                    continue;
                }
                Some(updated)
            }
            None => None,
            Some(_) => continue,
        };
        if emojis
            .iter()
            .any(|emoji: &RemoteEmojiTag| emoji.shortcode == shortcode)
        {
            continue;
        }
        emojis.push(RemoteEmojiTag {
            shortcode: shortcode.to_owned(),
            uri,
            image_url: image_url.to_string(),
            media_type,
            updated_at,
        });
        if emojis.len() == MAX_REMOTE_EMOJIS {
            break;
        }
    }
    emojis
}

fn value_includes_text(value: Option<&Value>, expected: &str) -> bool {
    match value {
        Some(Value::String(value)) => value == expected,
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(expected)),
        _ => false,
    }
}

fn parse_follow_decision(
    activity: &serde_json::Map<String, Value>,
    accepted: bool,
) -> Result<InboxActivity, InboxParseError> {
    let actor_uri = required_uri(activity.get("actor"))?;
    let object = activity.get("object").ok_or(InboxParseError::Activity)?;
    let (follow_uri, target_uri, nested_actor_uri) = match object {
        Value::String(_) => (required_uri(Some(object))?, None, None),
        Value::Object(object) => match object.get("type").and_then(Value::as_str) {
            Some("Follow") | None => (
                required_uri(object.get("id"))?,
                object
                    .get("object")
                    .map(|value| required_uri(Some(value)))
                    .transpose()?,
                object
                    .get("actor")
                    .map(|value| required_uri(Some(value)))
                    .transpose()?,
            ),
            Some(_) => return Ok(InboxActivity::Unsupported),
        },
        _ => return Err(InboxParseError::Activity),
    };
    Ok(if accepted {
        InboxActivity::Accept {
            actor_uri,
            follow_uri,
            target_uri,
            nested_actor_uri,
        }
    } else {
        InboxActivity::Reject {
            actor_uri,
            follow_uri,
            target_uri,
            nested_actor_uri,
        }
    })
}

fn parse_undo(activity: &serde_json::Map<String, Value>) -> Result<InboxActivity, InboxParseError> {
    let actor_uri = required_uri(activity.get("actor"))?;
    let object = activity.get("object").ok_or(InboxParseError::Activity)?;
    match object {
        Value::String(_) => Ok(InboxActivity::UndoReference {
            actor_uri,
            object_uri: required_uri(Some(object))?,
        }),
        Value::Object(object) => match object.get("type").and_then(Value::as_str) {
            Some("Follow") | None => Ok(InboxActivity::UndoFollow {
                actor_uri,
                follow_uri: required_uri(object.get("id"))?,
                target_uri: object
                    .get("object")
                    .map(|value| required_uri(Some(value)))
                    .transpose()?,
                nested_actor_uri: object
                    .get("actor")
                    .map(|value| required_uri(Some(value)))
                    .transpose()?,
            }),
            Some("Block") => Ok(InboxActivity::UndoBlock {
                actor_uri,
                block_uri: required_uri(object.get("id"))?,
                target_uri: object
                    .get("object")
                    .map(|value| required_uri(Some(value)))
                    .transpose()?,
                nested_actor_uri: object
                    .get("actor")
                    .map(|value| required_uri(Some(value)))
                    .transpose()?,
            }),
            Some("Like" | "Announce") => {
                let activity_uri = required_uri(object.get("id"))?;
                let object_uri = required_uri(object.get("object"))?;
                if let Some(nested_actor_uri) = object.get("actor")
                    && required_uri(Some(nested_actor_uri))? != actor_uri
                {
                    return Err(InboxParseError::Activity);
                }
                if object.get("type").and_then(Value::as_str) == Some("Like") {
                    Ok(InboxActivity::UndoLike {
                        actor_uri,
                        activity_uri,
                        object_uri,
                    })
                } else {
                    Ok(InboxActivity::UndoAnnounce {
                        actor_uri,
                        activity_uri,
                        object_uri,
                    })
                }
            }
            Some(_) => Ok(InboxActivity::Unsupported),
        },
        _ => Err(InboxParseError::Activity),
    }
}

fn required_text(value: Option<&Value>) -> Result<String, InboxParseError> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or(InboxParseError::Arguments)
}

fn required_uri(value: Option<&Value>) -> Result<String, InboxParseError> {
    let value = value_or_id(value).ok_or(InboxParseError::Activity)?;
    let url = Url::parse(&value).map_err(|_| InboxParseError::Activity)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(InboxParseError::Activity);
    }
    Ok(value)
}

fn first_uri(value: Option<&Value>) -> Result<String, InboxParseError> {
    match value {
        Some(Value::Array(values)) => required_uri(values.first()),
        _ => required_uri(value),
    }
}

fn optional_uri(value: Option<&Value>) -> Result<Option<String>, InboxParseError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => required_uri(Some(value)).map(Some),
    }
}

fn optional_conversation_uri(value: Option<&Value>) -> Result<Option<String>, InboxParseError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let uri = value_or_id(Some(value)).ok_or(InboxParseError::Activity)?;
    if let Some(tag_uri) = uri.strip_prefix("tag:") {
        return (!tag_uri.trim().is_empty())
            .then_some(uri)
            .ok_or(InboxParseError::Activity)
            .map(Some);
    }
    required_uri(Some(value)).map(Some)
}

fn audience_uri(value: Option<&Value>) -> Result<String, InboxParseError> {
    if value
        .and_then(Value::as_str)
        .is_some_and(|uri| matches!(uri, "as:Public" | "Public"))
    {
        return Ok(value.and_then(Value::as_str).unwrap_or_default().to_owned());
    }
    required_uri(value)
}

fn bounded_text(
    object: &serde_json::Map<String, Value>,
    field: &str,
    max_chars: usize,
) -> Result<Option<String>, InboxParseError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.chars().count() <= max_chars => Ok(Some(value.clone())),
        _ => Err(InboxParseError::Activity),
    }
}

fn value_or_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => (!value.trim().is_empty()).then(|| value.trim().to_owned()),
        Value::Object(value) => value_or_id(value.get("id").or_else(|| value.get("href"))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use serde_json::json;

    use super::{
        InboxActivity, InboxParseError, MAX_REMOTE_EMOJIS, MAX_REMOTE_TAGS, RemoteEmojiTag,
        parse_activity, parse_job_arguments, parse_note_emojis,
    };

    #[test]
    fn parses_the_durable_inbox_job_contract() {
        let job = parse_job_arguments(&json!({
            "body": "{\"type\":\"Follow\"}",
            "delivery_target_account_id": 42,
            "signature_key_id": "https://remote.example/users/alice#main-key",
            "remote_domain": "remote.example"
        }))
        .expect("the ingress payload should parse");

        assert_eq!(job.delivery_target_account_id, Some(42));
        assert_eq!(
            job.signature_key_id,
            "https://remote.example/users/alice#main-key"
        );
        assert_eq!(job.remote_domain, "remote.example");
    }

    #[test]
    fn parses_follow_and_uri_only_undo_activities() {
        let follow = parse_activity(
            r#"{"id":"https://remote.example/activities/1","type":"Follow","actor":{"id":"https://remote.example/users/alice"},"object":"https://local.example/users/bob"}"#,
        )
        .expect("Follow should parse");
        assert_eq!(
            follow,
            InboxActivity::Follow {
                activity_uri: "https://remote.example/activities/1".to_owned(),
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uri: "https://local.example/users/bob".to_owned(),
            }
        );

        let undo = parse_activity(
            r#"{"type":"Undo","actor":"https://remote.example/users/alice","object":"https://remote.example/activities/1"}"#,
        )
        .expect("URI-only Undo should parse");
        assert_eq!(
            undo,
            InboxActivity::UndoReference {
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uri: "https://remote.example/activities/1".to_owned(),
            }
        );
    }

    #[test]
    fn parses_block_and_embedded_block_undo_activities() {
        let block = parse_activity(
            r#"{"id":"https://remote.example/activities/block-1","type":"Block","actor":"https://remote.example/users/alice","object":"https://local.example/users/bob"}"#,
        )
        .expect("Block should parse");
        assert_eq!(
            block,
            InboxActivity::Block {
                activity_uri: "https://remote.example/activities/block-1".to_owned(),
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uri: "https://local.example/users/bob".to_owned(),
            }
        );

        let undo = parse_activity(
            r#"{"type":"Undo","actor":"https://remote.example/users/alice","object":{"type":"Block","id":"https://remote.example/activities/block-1","actor":"https://remote.example/users/alice","object":"https://local.example/users/bob"}}"#,
        )
        .expect("embedded Block Undo should parse");
        assert_eq!(
            undo,
            InboxActivity::UndoBlock {
                actor_uri: "https://remote.example/users/alice".to_owned(),
                block_uri: "https://remote.example/activities/block-1".to_owned(),
                target_uri: Some("https://local.example/users/bob".to_owned()),
                nested_actor_uri: Some("https://remote.example/users/alice".to_owned()),
            }
        );
    }

    #[test]
    fn parses_actor_update_and_delete_without_accepting_status_updates() {
        let update = parse_activity(
            r#"{"type":"Update","actor":"https://remote.example/users/alice","object":{"id":"https://remote.example/users/alice","type":"Person","preferredUsername":"alice","name":"Alice","summary":"Hello","url":"https://remote.example/@alice","inbox":"https://remote.example/inbox","outbox":"https://remote.example/outbox","followers":"https://remote.example/followers","following":"https://remote.example/following","endpoints":{"sharedInbox":"https://remote.example/shared"}}}"#,
        )
        .expect("actor Update should parse");
        assert!(matches!(
            update,
            InboxActivity::UpdateActor { actor_uri, .. }
                if actor_uri == "https://remote.example/users/alice"
        ));

        let delete = parse_activity(
            r#"{"type":"Delete","actor":"https://remote.example/users/alice","object":"https://remote.example/users/alice"}"#,
        )
        .expect("actor Delete should parse");
        assert_eq!(
            delete,
            InboxActivity::DeleteActor {
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uri: "https://remote.example/users/alice".to_owned(),
            }
        );

        assert!(matches!(
            parse_activity(
                r#"{"type":"Delete","actor":"https://remote.example/users/alice","object":"https://remote.example/statuses/1"}"#,
            )
            .expect("status Delete should parse"),
            InboxActivity::DeleteNote {
                actor_uri,
                object_uri,
                atom_uri: None,
                activity,
            } if actor_uri == "https://remote.example/users/alice"
                && object_uri == "https://remote.example/statuses/1"
                && activity["type"] == "Delete"
        ));
    }

    #[test]
    fn parses_note_create_update_and_atom_uri_delete() {
        let create = parse_activity(
            r#"{"id":"https://remote.example/activities/create-1","type":"Create","actor":"https://remote.example/users/alice","object":{"id":"https://remote.example/statuses/1","type":"Note","attributedTo":"https://remote.example/users/alice","published":"2026-08-25T12:00:00Z","content":"<p>Hello</p>","to":["https://www.w3.org/ns/activitystreams#Public"],"cc":[],"tag":[],"attachment":[]}}"#,
        )
        .expect("Note Create should parse");
        assert!(matches!(
            create,
            InboxActivity::CreateNote {
                activity_uri,
                actor_uri,
                ..
            } if activity_uri == "https://remote.example/activities/create-1"
                && actor_uri == "https://remote.example/users/alice"
        ));

        let update = parse_activity(
            r#"{"type":"Update","actor":"https://remote.example/users/alice","object":{"id":"https://remote.example/statuses/1","type":"Note","attributedTo":"https://remote.example/users/alice","updated":"2026-08-25T12:01:00Z","contentMap":{"en":"<p>Edited</p>"},"to":[],"cc":[]}}"#,
        )
        .expect("Note Update should parse");
        assert!(matches!(update, InboxActivity::UpdateNote { .. }));

        let delete = parse_activity(
            r#"{"type":"Delete","actor":"https://remote.example/users/alice","object":{"id":"https://remote.example/statuses/1","type":"Tombstone","atomUri":"https://remote.example/objects/1"}}"#,
        )
        .expect("Note Delete should parse");
        assert!(matches!(
            delete,
            InboxActivity::DeleteNote {
                actor_uri,
                object_uri,
                atom_uri: Some(atom_uri),
                activity,
            } if actor_uri == "https://remote.example/users/alice"
                && object_uri == "https://remote.example/statuses/1"
                && atom_uri == "https://remote.example/objects/1"
                && activity["type"] == "Delete"
        ));
    }

    #[test]
    fn parses_and_bounds_remote_emoji_metadata_without_trusting_its_domain() {
        let object = json!({
            "tag": [
                {"type": "Mention", "name": "@bob", "href": "https://elsewhere.example/@bob"},
                {"id": "https://remote.example/emojis/blobcat", "type": ["Emoji"],
                 "name": ":blobcat:", "updated": "2026-08-25T12:01:00Z",
                 "icon": {"type": "Image", "mediaType": "image/png",
                          "url": "https://cdn.example/blobcat.png"}},
                {"type": "Emoji", "name": "blobcat",
                 "icon": {"url": "https://cdn.example/duplicate.png"}},
                {"id": "https://attacker.example/emojis/spoof", "type": "Emoji", "name": "spoof",
                 "icon": {"url": "https://cdn.example/spoof.png"}},
                {"type": "Emoji", "name": "bad-name", "icon": {"url": "https://cdn.example/bad.png"}},
                {"type": "Emoji", "name": "jpeg", "icon": {"mediaType": "image/jpeg", "url": "https://cdn.example/bad.jpg"}}
            ]
        });

        assert_eq!(
            parse_note_emojis(&object, "https://remote.example/users/alice"),
            vec![RemoteEmojiTag {
                shortcode: "blobcat".to_owned(),
                uri: Some("https://remote.example/emojis/blobcat".to_owned()),
                image_url: "https://cdn.example/blobcat.png".to_owned(),
                media_type: Some("image/png".to_owned()),
                updated_at: Some(
                    DateTime::parse_from_rfc3339("2026-08-25T12:01:00Z")
                        .expect("timestamp")
                        .naive_utc()
                ),
            }]
        );
    }

    #[test]
    fn remote_emoji_limit_counts_accepted_emojis_not_preceding_tags() {
        let mut tags = (0..101)
            .map(|index| json!({"type": "Mention", "name": format!("@user{index}")}))
            .collect::<Vec<_>>();
        tags.extend((0..101).map(|index| {
            json!({
                "type": "Emoji",
                "name": format!(":emoji_{index}:"),
                "icon": {"url": format!("https://cdn.example/emoji-{index}.png")}
            })
        }));

        let emojis = parse_note_emojis(&json!({"tag": tags}), "https://remote.example/users/alice");

        assert_eq!(emojis.len(), MAX_REMOTE_EMOJIS);
        assert_eq!(
            emojis.first().map(|emoji| emoji.shortcode.as_str()),
            Some("emoji_0")
        );
        assert_eq!(
            emojis.last().map(|emoji| emoji.shortcode.as_str()),
            Some("emoji_99")
        );
    }

    #[test]
    fn remote_emoji_tag_traversal_remains_bounded() {
        let mut tags = (0..MAX_REMOTE_TAGS)
            .map(|index| json!({"type": "Mention", "name": format!("@user{index}")}))
            .collect::<Vec<_>>();
        tags.push(json!({
            "type": "Emoji",
            "name": ":too_late:",
            "icon": {"url": "https://cdn.example/too-late.png"}
        }));

        assert!(
            parse_note_emojis(&json!({"tag": tags}), "https://remote.example/users/alice")
                .is_empty()
        );
    }

    #[test]
    fn parses_uri_only_note_create_as_a_durable_resolution_contract() {
        let create = parse_activity(
            r#"{"id":"https://remote.example/activities/create-reference","type":"Create","actor":"https://remote.example/users/alice","object":"https://remote.example/statuses/1","to":["https://local.example/users/bob"],"cc":"https://www.w3.org/ns/activitystreams#Public"}"#,
        )
        .expect("URI-only Create should parse");

        assert_eq!(
            create,
            InboxActivity::CreateNoteReference {
                activity_uri: "https://remote.example/activities/create-reference".to_owned(),
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uri: "https://remote.example/statuses/1".to_owned(),
                to: vec!["https://local.example/users/bob".to_owned()],
                cc: vec!["https://www.w3.org/ns/activitystreams#Public".to_owned()],
                activity: json!({
                    "id": "https://remote.example/activities/create-reference",
                    "type": "Create",
                    "actor": "https://remote.example/users/alice",
                    "object": "https://remote.example/statuses/1",
                    "to": ["https://local.example/users/bob"],
                    "cc": "https://www.w3.org/ns/activitystreams#Public"
                }),
            }
        );
    }

    #[test]
    fn parses_scalar_and_null_note_audiences() {
        let create = parse_activity(
            r#"{"id":"https://remote.example/activities/scalar-audience-create","type":"Create","actor":"https://remote.example/users/alice","object":{"id":"https://remote.example/statuses/scalar-audience","type":"Note","attributedTo":["https://remote.example/users/alice"],"content":"<p>Hello</p>","to":"as:Public","cc":"Public","tag":[],"attachment":[]}}"#,
        )
        .expect("scalar and null audiences should parse");
        assert!(matches!(create, InboxActivity::CreateNote { .. }));
    }

    #[test]
    fn parses_like_announce_and_typed_undo_activities() {
        let like = parse_activity(
            r#"{"id":"https://remote.example/activities/like-1","type":"Like","actor":"https://remote.example/users/alice","object":"https://local.example/statuses/1"}"#,
        )
        .expect("Like should parse");
        assert_eq!(
            like,
            InboxActivity::Like {
                activity_uri: "https://remote.example/activities/like-1".to_owned(),
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uri: "https://local.example/statuses/1".to_owned(),
            }
        );

        let announce = parse_activity(
            r#"{"id":"https://remote.example/activities/announce-1","type":"Announce","actor":"https://remote.example/users/alice","object":{"id":"https://local.example/statuses/1"},"to":"https://www.w3.org/ns/activitystreams#Public","cc":null,"published":"2026-08-25T12:00:00Z"}"#,
        )
        .expect("Announce should parse");
        assert_eq!(
            announce,
            InboxActivity::Announce {
                activity_uri: "https://remote.example/activities/announce-1".to_owned(),
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uri: "https://local.example/statuses/1".to_owned(),
                embedded_note: None,
                to: vec!["https://www.w3.org/ns/activitystreams#Public".to_owned()],
                cc: Vec::new(),
                published_at: Some("2026-08-25T12:00:00Z".to_owned()),
            }
        );

        let undo = parse_activity(
            r#"{"type":"Undo","actor":"https://remote.example/users/alice","object":{"type":"Like","id":"https://remote.example/activities/like-1","actor":"https://remote.example/users/alice","object":"https://local.example/statuses/1"}}"#,
        )
        .expect("typed Like Undo should parse");
        assert_eq!(
            undo,
            InboxActivity::UndoLike {
                actor_uri: "https://remote.example/users/alice".to_owned(),
                activity_uri: "https://remote.example/activities/like-1".to_owned(),
                object_uri: "https://local.example/statuses/1".to_owned(),
            }
        );

        let uri_only_undo = parse_activity(
            r#"{"type":"Undo","actor":"https://remote.example/users/alice","object":"https://remote.example/activities/announce-1"}"#,
        )
        .expect("URI-only Undo should parse");
        assert_eq!(
            uri_only_undo,
            InboxActivity::UndoReference {
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uri: "https://remote.example/activities/announce-1".to_owned(),
            }
        );
    }

    #[test]
    fn preserves_embedded_self_boost_notes() {
        let announce = parse_activity(
            r#"{"id":"https://remote.example/activities/announce-embedded","type":"Announce","actor":"https://remote.example/users/alice","object":{"id":"https://remote.example/statuses/1","type":"Note","attributedTo":"https://remote.example/users/alice","content":"<p>Embedded</p>","to":["https://www.w3.org/ns/activitystreams#Public"],"cc":[],"tag":[],"attachment":[]}}"#,
        )
        .expect("embedded self-boost should parse");

        let InboxActivity::Announce {
            embedded_note: Some(note),
            object_uri,
            ..
        } = announce
        else {
            panic!("the embedded Note should be retained");
        };
        assert_eq!(object_uri, "https://remote.example/statuses/1");
        assert_eq!(note["type"], "Note");
        assert_eq!(note["attributedTo"], "https://remote.example/users/alice");
    }

    #[test]
    fn ignores_foreign_embedded_notes_and_keeps_the_target_uri() {
        let announce = parse_activity(
            r#"{"id":"https://remote.example/activities/announce-foreign","type":"Announce","actor":"https://remote.example/users/alice","object":{"id":"https://remote.example/statuses/1","type":"Note","attributedTo":"https://remote.example/users/bob","content":"<p>Foreign</p>"}}"#,
        )
        .expect("foreign embedded Note should use remote resolution");

        let InboxActivity::Announce {
            embedded_note,
            object_uri,
            ..
        } = announce
        else {
            panic!("the activity should be an Announce");
        };
        assert_eq!(embedded_note, None);
        assert_eq!(object_uri, "https://remote.example/statuses/1");
    }

    #[test]
    fn parses_scalar_mastodon_note_metadata_and_link_urls() {
        let create = parse_activity(
            r##"{"id":"https://remote.example/activities/scalar-note","type":"Create","actor":"https://remote.example/users/alice","object":{"id":"https://remote.example/statuses/scalar-note","type":"Note","attributedTo":"https://remote.example/users/alice","url":{"type":"Link","href":"https://remote.example/@alice/1"},"conversation":"tag:remote.example,2026-08-25:conversation-1","content":"<p>Scalar metadata</p>","to":"https://www.w3.org/ns/activitystreams#Public","cc":[],"tag":{"type":"Hashtag","name":"#rust"},"attachment":{"type":"Document","mediaType":"image/jpeg","url":"https://remote.example/media/1.jpg"}}}"##,
        )
        .expect("Mastodon scalar metadata should parse");
        assert!(matches!(create, InboxActivity::CreateNote { .. }));
    }

    #[test]
    fn parses_accept_and_reject_follow_decisions() {
        let accept = parse_activity(
            r#"{"type":"Accept","actor":"https://remote.example/users/bob","object":{"type":"Follow","id":"https://local.example/activities/follow-1","actor":"https://local.example/users/alice","object":"https://remote.example/users/bob"}}"#,
        )
        .expect("Accept should parse");
        assert_eq!(
            accept,
            InboxActivity::Accept {
                actor_uri: "https://remote.example/users/bob".to_owned(),
                follow_uri: "https://local.example/activities/follow-1".to_owned(),
                target_uri: Some("https://remote.example/users/bob".to_owned()),
                nested_actor_uri: Some("https://local.example/users/alice".to_owned()),
            }
        );

        let reject = parse_activity(
            r#"{"type":"Reject","actor":"https://remote.example/users/bob","object":"https://local.example/activities/follow-1"}"#,
        )
        .expect("Reject should parse");
        assert_eq!(
            reject,
            InboxActivity::Reject {
                actor_uri: "https://remote.example/users/bob".to_owned(),
                follow_uri: "https://local.example/activities/follow-1".to_owned(),
                target_uri: None,
                nested_actor_uri: None,
            }
        );
    }

    #[test]
    fn parses_account_only_flag_activities_with_bounded_comments() {
        let flag = parse_activity(
            r#"{"id":"https://remote.example/activities/flag-1","type":"Flag","actor":"https://remote.example/users/alice","object":"https://local.example/users/bob","content":"Please review this account"}"#,
        )
        .expect("Flag should parse");
        assert_eq!(
            flag,
            InboxActivity::Flag {
                activity_uri: Some("https://remote.example/activities/flag-1".to_owned()),
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uris: vec!["https://local.example/users/bob".to_owned()],
                comment: "Please review this account".to_owned(),
            }
        );

        let long_comment = "a".repeat(5_001);
        let body = serde_json::json!({
            "type": "Flag",
            "actor": "https://remote.example/users/alice",
            "object": ["https://local.example/users/bob"],
            "content": long_comment,
        });
        let flag = parse_activity(&body.to_string()).expect("long Flag comments should be bounded");
        assert!(matches!(
            flag,
            InboxActivity::Flag { comment, .. } if comment.chars().count() == 5_000
        ));

        let non_http_flag = parse_activity(
            r#"{"id":"tag:remote.example,2026:flag-1","type":"Flag","actor":"https://remote.example/users/alice","object":["tag:local.example,2026:status-1",{"id":"https://local.example/users/bob"}]}"#,
        )
        .expect("Flag object identifiers should be retained without requiring HTTP URLs");
        assert_eq!(
            non_http_flag,
            InboxActivity::Flag {
                activity_uri: Some("tag:remote.example,2026:flag-1".to_owned()),
                actor_uri: "https://remote.example/users/alice".to_owned(),
                object_uris: vec![
                    "tag:local.example,2026:status-1".to_owned(),
                    "https://local.example/users/bob".to_owned(),
                ],
                comment: String::new(),
            }
        );
    }

    #[test]
    fn preserves_embedded_follow_identity_for_undo_validation() {
        let undo = parse_activity(
            r#"{"type":"Undo","actor":"https://remote.example/users/alice","object":{"type":"Follow","id":"https://remote.example/activities/1","actor":"https://remote.example/users/alice","object":"https://local.example/users/bob"}}"#,
        )
        .expect("embedded Undo should parse");

        assert_eq!(
            undo,
            InboxActivity::UndoFollow {
                actor_uri: "https://remote.example/users/alice".to_owned(),
                follow_uri: "https://remote.example/activities/1".to_owned(),
                target_uri: Some("https://local.example/users/bob".to_owned()),
                nested_actor_uri: Some("https://remote.example/users/alice".to_owned()),
            }
        );
    }

    #[test]
    fn unsupported_activity_types_are_acknowledged_without_mutation() {
        assert_eq!(
            parse_activity(r#"{"type":"Move","actor":"https://remote.example/users/alice"}"#)
                .expect("unsupported activities should parse"),
            InboxActivity::Unsupported
        );
    }

    #[test]
    fn malformed_activity_is_permanent_input_error() {
        assert_eq!(
            parse_activity(r#"{"type":"Follow","actor":"not-a-url"}"#),
            Err(InboxParseError::Activity)
        );
    }
}
