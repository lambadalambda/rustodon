use serde::Serialize;
use serde_json::Value;

use crate::mastodon::{OAuthScopes, READ_NOTIFICATIONS, READ_STATUSES};

pub const STREAM_EVENT_KIND: &str = "rustodon.mastodon.stream_event";
pub const SYSTEM_KILL_EVENT: &str = "kill";
pub const TOKEN_KILL_EVENT: &str = "kill:token";
pub const STATUS_UPDATE_NOTIFICATION_EVENT: &str = "status.update:notification";
pub const STREAM_EVENT_BATCH_SIZE: i64 = 128;

#[must_use]
pub fn event_logical_key(account_id: i64, event: &str, object_id: i64, version: i64) -> String {
    format!("stream:{account_id}:{event}:{object_id}:{version}")
}

#[must_use]
pub fn media_event_logical_key(
    account_id: i64,
    event: &str,
    object_id: i64,
    media_id: i64,
) -> String {
    format!("stream:{account_id}:{event}:{object_id}:media:{media_id}")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamName {
    User,
    UserNotification,
    Direct,
}

impl StreamName {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "user:notification" => Some(Self::UserNotification),
            "direct" => Some(Self::Direct),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::UserNotification => "user:notification",
            Self::Direct => "direct",
        }
    }

    #[must_use]
    pub fn permits(self, scopes: &OAuthScopes) -> bool {
        scopes.permits(match self {
            Self::User | Self::Direct => READ_STATUSES,
            Self::UserNotification => READ_NOTIFICATIONS,
        })
    }

    #[must_use]
    pub fn includes_notifications(self, scopes: &OAuthScopes) -> bool {
        self == Self::UserNotification
            || (self == Self::User
                && (scopes.contains("read") || scopes.contains("read:notifications")))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientCommand {
    Subscribe(StreamName),
    Unsubscribe(StreamName),
}

impl ClientCommand {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        let command = value.get("type")?.as_str()?;
        let stream = value.get("stream")?;
        let stream = match stream {
            Value::String(stream) => stream.as_str(),
            Value::Array(streams) => streams.first()?.as_str()?,
            _ => return None,
        };
        let stream = StreamName::parse(stream)?;
        match command {
            "subscribe" => Some(Self::Subscribe(stream)),
            "unsubscribe" => Some(Self::Unsubscribe(stream)),
            _ => None,
        }
    }
}

#[derive(Serialize)]
struct EventMessage<'a> {
    stream: [&'a str; 1],
    event: &'a str,
    payload: &'a str,
}

///
/// # Panics
///
/// This cannot panic because the envelope contains only serializable primitive fields.
#[must_use]
pub fn event_message(stream: StreamName, event: &str, payload: &str) -> String {
    serde_json::to_string(&EventMessage {
        stream: [stream.as_str()],
        event,
        payload,
    })
    .expect("stream event envelope is serializable")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamEvent {
    pub id: i64,
    pub account_id: i64,
    pub event: String,
    pub object_id: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        ClientCommand, StreamName, event_logical_key, event_message, media_event_logical_key,
    };
    use crate::mastodon::OAuthScopes;

    #[test]
    fn parses_subscribe_and_unsubscribe_commands() {
        assert_eq!(
            ClientCommand::parse(r#"{"type":"subscribe","stream":"user"}"#),
            Some(ClientCommand::Subscribe(StreamName::User))
        );
        assert_eq!(
            ClientCommand::parse(r#"{"type":"unsubscribe","stream":["user:notification"]}"#),
            Some(ClientCommand::Unsubscribe(StreamName::UserNotification))
        );
        assert_eq!(
            ClientCommand::parse(r#"{"type":"subscribe","stream":"direct"}"#),
            Some(ClientCommand::Subscribe(StreamName::Direct))
        );
    }

    #[test]
    fn ignores_malformed_and_unknown_commands() {
        assert_eq!(ClientCommand::parse("not json"), None);
        assert_eq!(
            ClientCommand::parse(r#"{"type":"subscribe","stream":"public"}"#),
            None
        );
        assert_eq!(
            ClientCommand::parse(r#"{"type":"ping","stream":"user"}"#),
            None
        );
    }

    #[test]
    fn serializes_the_mastodon_event_envelope() {
        assert_eq!(
            event_message(StreamName::User, "update", r#"{"id":"42"}"#),
            r#"{"stream":["user"],"event":"update","payload":"{\"id\":\"42\"}"}"#
        );
        assert_eq!(
            event_message(StreamName::UserNotification, "delete", "42"),
            r#"{"stream":["user:notification"],"event":"delete","payload":"42"}"#
        );
        assert_eq!(
            event_message(StreamName::Direct, "conversation", "42"),
            r#"{"stream":["direct"],"event":"conversation","payload":"42"}"#
        );
    }

    #[test]
    fn applies_mastodon_user_stream_scope_rules() {
        let statuses = OAuthScopes::parse(Some("read:statuses"));
        assert!(StreamName::User.permits(&statuses));
        assert!(!StreamName::User.includes_notifications(&statuses));
        assert!(!StreamName::UserNotification.permits(&statuses));

        let notifications = OAuthScopes::parse(Some("read:notifications"));
        assert!(!StreamName::User.permits(&notifications));
        assert!(StreamName::UserNotification.permits(&notifications));
        assert!(StreamName::User.includes_notifications(&notifications));

        assert!(StreamName::Direct.permits(&statuses));
        assert!(!StreamName::Direct.includes_notifications(&statuses));
    }

    #[test]
    fn event_keys_are_stable_per_recipient_version() {
        let first = event_logical_key(7, "status.update", 42, 3);
        assert_eq!(first, event_logical_key(7, "status.update", 42, 3));
        assert_ne!(first, event_logical_key(8, "status.update", 42, 3));
        assert_ne!(first, event_logical_key(7, "status.update", 42, 4));
    }

    #[test]
    fn media_event_keys_cannot_collide_with_semantic_versions() {
        let semantic = event_logical_key(7, "status.update", 42, 3);
        let media = media_event_logical_key(7, "status.update", 42, 3);
        assert_eq!(media, "stream:7:status.update:42:media:3");
        assert_ne!(semantic, media);
        assert_eq!(media, media_event_logical_key(7, "status.update", 42, 3));
        assert_ne!(media, media_event_logical_key(7, "status.update", 42, 4));
    }
}
