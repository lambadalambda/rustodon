use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mastodon::{OAuthScopes, READ_NOTIFICATIONS, READ_STATUSES, normalize_hashtag};

pub const STREAM_EVENT_KIND: &str = "rustodon.mastodon.stream_event";
pub const SYSTEM_KILL_EVENT: &str = "kill";
pub const TOKEN_KILL_EVENT: &str = "kill:token";
pub const STATUS_UPDATE_NOTIFICATION_EVENT: &str = "status.update:notification";
pub const STREAM_EVENT_BATCH_SIZE: i64 = 128;
pub const STREAM_REPLAY_TRANSITION_EVENTS: i64 = 128;
pub const STREAM_REPLAY_UPDATE_EVENTS: i64 = 40;
pub const STREAM_HISTORY_MAX_EVENTS: i64 = 20_000;
pub const STREAM_HISTORY_MAX_AGE_HOURS: i64 = 24;
pub const STREAM_MAX_SUBSCRIPTIONS: usize = 100;
pub const STREAM_MAX_PARAMETER_BYTES: usize = 255;

#[must_use]
pub fn event_logical_key(account_id: i64, event: &str, object_id: i64, version: i64) -> String {
    format!("stream:{account_id}:{event}:{object_id}:{version}")
}

#[must_use]
pub fn global_event_logical_key(event: &str, object_id: i64, version: i64) -> String {
    format!("stream:global:{event}:{object_id}:{version}")
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StreamName {
    User,
    UserNotification,
    Direct,
    Public,
    PublicMedia,
    PublicLocal,
    PublicLocalMedia,
    PublicRemote,
    PublicRemoteMedia,
    Hashtag,
    HashtagLocal,
    List,
}

impl StreamName {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "user:notification" => Some(Self::UserNotification),
            "direct" => Some(Self::Direct),
            "public" => Some(Self::Public),
            "public:media" => Some(Self::PublicMedia),
            "public:local" => Some(Self::PublicLocal),
            "public:local:media" => Some(Self::PublicLocalMedia),
            "public:remote" => Some(Self::PublicRemote),
            "public:remote:media" => Some(Self::PublicRemoteMedia),
            "hashtag" => Some(Self::Hashtag),
            "hashtag:local" => Some(Self::HashtagLocal),
            "list" => Some(Self::List),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::UserNotification => "user:notification",
            Self::Direct => "direct",
            Self::Public => "public",
            Self::PublicMedia => "public:media",
            Self::PublicLocal => "public:local",
            Self::PublicLocalMedia => "public:local:media",
            Self::PublicRemote => "public:remote",
            Self::PublicRemoteMedia => "public:remote:media",
            Self::Hashtag => "hashtag",
            Self::HashtagLocal => "hashtag:local",
            Self::List => "list",
        }
    }

    #[must_use]
    pub fn permits(self, scopes: &OAuthScopes) -> bool {
        scopes.permits(match self {
            Self::UserNotification => READ_NOTIFICATIONS,
            Self::User
            | Self::Direct
            | Self::Public
            | Self::PublicMedia
            | Self::PublicLocal
            | Self::PublicLocalMedia
            | Self::PublicRemote
            | Self::PublicRemoteMedia
            | Self::Hashtag
            | Self::HashtagLocal
            | Self::List => READ_STATUSES,
        })
    }

    #[must_use]
    pub fn includes_notifications(self, scopes: &OAuthScopes) -> bool {
        self == Self::UserNotification
            || (self == Self::User
                && (scopes.contains("read") || scopes.contains("read:notifications")))
    }

    #[must_use]
    pub const fn is_timeline(self) -> bool {
        matches!(
            self,
            Self::Public
                | Self::PublicMedia
                | Self::PublicLocal
                | Self::PublicLocalMedia
                | Self::PublicRemote
                | Self::PublicRemoteMedia
                | Self::Hashtag
                | Self::HashtagLocal
                | Self::List
        )
    }
}

#[derive(Clone, Debug)]
pub struct Subscription {
    stream: StreamName,
    wire_parameter: Option<String>,
    canonical_parameter: Option<String>,
}

impl Subscription {
    #[must_use]
    pub fn new(stream: StreamName, parameter: Option<String>) -> Self {
        let canonical_parameter = parameter.as_ref().map(|parameter| {
            if matches!(stream, StreamName::Hashtag | StreamName::HashtagLocal) {
                normalize_hashtag(parameter)
            } else {
                parameter.clone()
            }
        });
        Self {
            stream,
            wire_parameter: parameter,
            canonical_parameter,
        }
    }

    #[must_use]
    pub const fn stream(&self) -> StreamName {
        self.stream
    }

    /// Returns the canonical parameter used for membership and authorization.
    #[must_use]
    pub fn parameter(&self) -> Option<&str> {
        self.canonical_parameter.as_deref()
    }

    #[must_use]
    pub const fn is_timeline(&self) -> bool {
        self.stream.is_timeline()
    }

    /// Returns the exact client identifier used by Mastodon's case-sensitive demultiplexer.
    #[must_use]
    pub fn identifier(&self) -> Vec<&str> {
        std::iter::once(self.stream.as_str())
            .chain(self.wire_parameter.iter().map(String::as_str))
            .collect()
    }
}

impl PartialEq for Subscription {
    fn eq(&self, other: &Self) -> bool {
        self.stream == other.stream && self.canonical_parameter == other.canonical_parameter
    }
}

impl Eq for Subscription {}

impl PartialOrd for Subscription {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Subscription {
    fn cmp(&self, other: &Self) -> Ordering {
        (&self.stream, &self.canonical_parameter).cmp(&(&other.stream, &other.canonical_parameter))
    }
}

impl Hash for Subscription {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.stream.hash(state);
        self.canonical_parameter.hash(state);
    }
}

impl From<StreamName> for Subscription {
    fn from(stream: StreamName) -> Self {
        Self::new(stream, None)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientCommand {
    Subscribe(Subscription),
    Unsubscribe(Subscription),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamError {
    pub status: Option<u16>,
    pub message: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedCommand {
    Command(ClientCommand),
    Ignore,
    Reject(StreamError),
}

impl ClientCommand {
    #[must_use]
    pub fn parse(text: &str) -> ParsedCommand {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return ParsedCommand::Ignore;
        };
        let Some(command) = value.get("type").and_then(Value::as_str) else {
            return ParsedCommand::Ignore;
        };
        if !matches!(command, "subscribe" | "unsubscribe") {
            return ParsedCommand::Ignore;
        }
        let reject = |status, message| {
            ParsedCommand::Reject(if command == "unsubscribe" {
                StreamError {
                    status: None,
                    message: "Error unsubscribing from channel",
                }
            } else {
                StreamError {
                    status: Some(status),
                    message,
                }
            })
        };
        let stream = value.get("stream");
        let stream = match stream {
            Some(Value::String(stream)) => Some(stream.as_str()),
            Some(Value::Array(streams)) => streams.first().and_then(Value::as_str),
            _ => None,
        };
        let Some(stream) = stream.and_then(StreamName::parse) else {
            return reject(400, "Unknown stream type");
        };
        let parameter =
            match stream {
                StreamName::Hashtag | StreamName::HashtagLocal => {
                    let Some(tag) = value
                        .get("tag")
                        .and_then(Value::as_str)
                        .filter(|tag| tag.len() <= STREAM_MAX_PARAMETER_BYTES)
                        .filter(|tag| !normalize_hashtag(tag).is_empty())
                    else {
                        return reject(400, "Missing tag name parameter");
                    };
                    Some(tag.to_owned())
                }
                StreamName::List => {
                    let Some(list) = value.get("list").and_then(Value::as_str).filter(|list| {
                        !list.is_empty() && list.len() <= STREAM_MAX_PARAMETER_BYTES
                    }) else {
                        return reject(400, "Missing list name parameter");
                    };
                    Some(list.to_owned())
                }
                _ => None,
            };
        let subscription = Subscription::new(stream, parameter);
        ParsedCommand::Command(match command {
            "subscribe" => Self::Subscribe(subscription),
            "unsubscribe" => Self::Unsubscribe(subscription),
            _ => unreachable!("command was checked above"),
        })
    }
}

#[derive(Serialize)]
struct EventMessage<'a> {
    stream: Vec<&'a str>,
    event: &'a str,
    payload: &'a str,
}

///
/// # Panics
///
/// This cannot panic because the envelope contains only serializable primitive fields.
#[must_use]
pub fn event_message(subscription: &Subscription, event: &str, payload: &str) -> String {
    serde_json::to_string(&EventMessage {
        stream: subscription.identifier(),
        event,
        payload,
    })
    .expect("stream event envelope is serializable")
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TimelineListRoute {
    pub account_id: i64,
    pub list_id: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TimelineRouteSnapshot {
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub hashtag: bool,
    #[serde(default)]
    pub local: bool,
    pub had_media: bool,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub lists: Vec<TimelineListRoute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamEvent {
    pub id: i64,
    /// Zero denotes an audience-independent status lifecycle event.
    pub account_id: i64,
    pub event: String,
    pub object_id: i64,
    pub before: Option<TimelineRouteSnapshot>,
    pub after: Option<TimelineRouteSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::{
        ClientCommand, ParsedCommand, StreamName, Subscription, TimelineRouteSnapshot,
        event_logical_key, event_message, global_event_logical_key, media_event_logical_key,
    };
    use crate::mastodon::OAuthScopes;

    fn command(text: &str) -> ClientCommand {
        let ParsedCommand::Command(command) = ClientCommand::parse(text) else {
            panic!("expected parsed command");
        };
        command
    }

    #[test]
    fn parses_all_bundled_timeline_subscriptions() {
        for stream in [
            "public",
            "public:media",
            "public:local",
            "public:local:media",
            "public:remote",
            "public:remote:media",
        ] {
            assert_eq!(
                command(&format!(r#"{{"type":"subscribe","stream":"{stream}"}}"#)),
                ClientCommand::Subscribe(Subscription::new(
                    StreamName::parse(stream).expect("known stream"),
                    None,
                ))
            );
        }
        assert_eq!(
            command(r#"{"type":"subscribe","stream":"hashtag","tag":"RustLang"}"#),
            ClientCommand::Subscribe(Subscription::new(
                StreamName::Hashtag,
                Some("RustLang".to_owned()),
            ))
        );
        let ClientCommand::Subscribe(hashtag) =
            command(r#"{"type":"subscribe","stream":"hashtag","tag":"RustLang"}"#)
        else {
            unreachable!();
        };
        assert_eq!(hashtag.parameter(), Some("rustlang"));
        assert_eq!(hashtag.identifier(), vec!["hashtag", "RustLang"]);
        assert_eq!(
            command(r#"{"type":"subscribe","stream":"hashtag:local","tag":"rust"}"#),
            ClientCommand::Subscribe(Subscription::new(
                StreamName::HashtagLocal,
                Some("rust".to_owned()),
            ))
        );
        assert_eq!(
            command(r#"{"type":"subscribe","stream":"list","list":"42"}"#),
            ClientCommand::Subscribe(Subscription::new(StreamName::List, Some("42".to_owned()),))
        );
    }

    #[test]
    fn parses_existing_subscribe_and_unsubscribe_commands() {
        assert_eq!(
            command(r#"{"type":"subscribe","stream":"user"}"#),
            ClientCommand::Subscribe(StreamName::User.into())
        );
        assert_eq!(
            command(r#"{"type":"unsubscribe","stream":["user:notification"]}"#),
            ClientCommand::Unsubscribe(StreamName::UserNotification.into())
        );
        assert_eq!(
            command(r#"{"type":"subscribe","stream":"direct"}"#),
            ClientCommand::Subscribe(StreamName::Direct.into())
        );
    }

    #[test]
    fn distinguishes_ignored_commands_from_protocol_errors() {
        assert_eq!(ClientCommand::parse("not json"), ParsedCommand::Ignore);
        assert_eq!(
            ClientCommand::parse(r#"{"type":"ping","stream":"user"}"#),
            ParsedCommand::Ignore
        );
        for (input, status, message) in [
            (
                r#"{"type":"subscribe","stream":"nope"}"#,
                400,
                "Unknown stream type",
            ),
            (
                r#"{"type":"subscribe","stream":"hashtag","tag":"---"}"#,
                400,
                "Missing tag name parameter",
            ),
            (
                r#"{"type":"subscribe","stream":"hashtag"}"#,
                400,
                "Missing tag name parameter",
            ),
            (
                r#"{"type":"subscribe","stream":"list"}"#,
                400,
                "Missing list name parameter",
            ),
        ] {
            let ParsedCommand::Reject(error) = ClientCommand::parse(input) else {
                panic!("expected rejection");
            };
            assert_eq!((error.status, error.message), (Some(status), message));
        }

        for input in [
            r#"{"type":"unsubscribe","stream":"nope"}"#,
            r#"{"type":"unsubscribe","stream":"hashtag"}"#,
            r#"{"type":"unsubscribe","stream":"list"}"#,
        ] {
            let ParsedCommand::Reject(error) = ClientCommand::parse(input) else {
                panic!("expected malformed unsubscribe rejection");
            };
            assert_eq!(error.status, None);
            assert_eq!(error.message, "Error unsubscribing from channel");
        }

        let oversized = format!(
            r#"{{"type":"subscribe","stream":"hashtag","tag":"{}"}}"#,
            "a".repeat(super::STREAM_MAX_PARAMETER_BYTES + 1)
        );
        assert_eq!(
            ClientCommand::parse(&oversized),
            ParsedCommand::Reject(super::StreamError {
                status: Some(400),
                message: "Missing tag name parameter",
            })
        );
    }

    #[test]
    fn serializes_static_and_parameterized_mastodon_event_envelopes() {
        assert_eq!(
            event_message(&StreamName::User.into(), "update", r#"{"id":"42"}"#),
            r#"{"stream":["user"],"event":"update","payload":"{\"id\":\"42\"}"}"#
        );
        assert_eq!(
            event_message(
                &Subscription::new(StreamName::Hashtag, Some("RustLang".to_owned())),
                "delete",
                "42",
            ),
            r#"{"stream":["hashtag","RustLang"],"event":"delete","payload":"42"}"#
        );
        assert_eq!(
            event_message(
                &Subscription::new(StreamName::List, Some("7".to_owned())),
                "update",
                "{}",
            ),
            r#"{"stream":["list","7"],"event":"update","payload":"{}"}"#
        );
    }

    #[test]
    fn hashtag_identity_is_canonical_but_wire_identifier_preserves_client_case() {
        let ClientCommand::Subscribe(subscribe) =
            command(r#"{"type":"subscribe","stream":"hashtag","tag":"RustLang"}"#)
        else {
            unreachable!();
        };
        let ClientCommand::Unsubscribe(unsubscribe) =
            command(r#"{"type":"unsubscribe","stream":"hashtag","tag":"rustlang"}"#)
        else {
            unreachable!();
        };

        assert_eq!(subscribe, unsubscribe);
        assert_eq!(subscribe.parameter(), Some("rustlang"));
        assert_eq!(subscribe.identifier(), vec!["hashtag", "RustLang"]);
        assert_eq!(
            event_message(&subscribe, "delete", "42"),
            r#"{"stream":["hashtag","RustLang"],"event":"delete","payload":"42"}"#
        );
    }

    #[test]
    fn legacy_timeline_snapshots_default_new_routing_flags_to_false() {
        let snapshot: TimelineRouteSnapshot = serde_json::from_str(
            r#"{"had_media":true,"language":"en","tags":["rust"],"lists":[]}"#,
        )
        .expect("legacy snapshot remains readable");

        assert!(!snapshot.public);
        assert!(!snapshot.hashtag);
        assert!(!snapshot.local);
        assert!(snapshot.had_media);
    }

    #[test]
    fn applies_stream_scope_rules() {
        let statuses = OAuthScopes::parse(Some("read:statuses"));
        for stream in [
            StreamName::User,
            StreamName::Direct,
            StreamName::Public,
            StreamName::PublicMedia,
            StreamName::PublicLocal,
            StreamName::PublicLocalMedia,
            StreamName::PublicRemote,
            StreamName::PublicRemoteMedia,
            StreamName::Hashtag,
            StreamName::HashtagLocal,
            StreamName::List,
        ] {
            assert!(stream.permits(&statuses), "{}", stream.as_str());
        }
        assert!(!StreamName::User.includes_notifications(&statuses));
        assert!(!StreamName::UserNotification.permits(&statuses));

        let notifications = OAuthScopes::parse(Some("read:notifications"));
        assert!(!StreamName::Public.permits(&notifications));
        assert!(StreamName::UserNotification.permits(&notifications));
        assert!(StreamName::User.includes_notifications(&notifications));
    }

    #[test]
    fn event_keys_are_stable_per_recipient_and_global_version() {
        let first = event_logical_key(7, "status.update", 42, 3);
        assert_eq!(first, event_logical_key(7, "status.update", 42, 3));
        assert_ne!(first, event_logical_key(8, "status.update", 42, 3));
        assert_ne!(first, event_logical_key(7, "status.update", 42, 4));
        assert_eq!(
            global_event_logical_key("status.update", 42, 3),
            "stream:global:status.update:42:3"
        );
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
