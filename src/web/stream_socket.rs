//! `WebSocket` streaming: authentication, subscriptions, replay and event delivery.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

pub(super) async fn streaming(
    State(state): State<WebState>,
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    uri: Uri,
    RawQuery(query): RawQuery,
) -> Response<Body> {
    let (auth_headers, websocket_protocol) = streaming_credentials(&headers, query.as_deref());
    let authenticated = match state
        .authenticator
        .authenticate(&auth_headers, NO_SCOPE)
        .await
    {
        Ok(authenticated) => authenticated,
        Err(OAuthAuthenticationError::OAuth(error)) => {
            return error.into_http_response().map(Body::from);
        }
        Err(OAuthAuthenticationError::Repository(_)) => return internal_error(),
    };
    if let Err(error) = authenticated.require_user() {
        return error.into_http_response().map(Body::from);
    }
    let Some(queue) = state.queue.clone() else {
        return internal_error();
    };
    let Ok(cursor) = queue.stream_cursor().await else {
        return internal_error();
    };
    let authenticated = match state
        .authenticator
        .authenticate(&auth_headers, NO_SCOPE)
        .await
    {
        Ok(authenticated) => authenticated,
        Err(OAuthAuthenticationError::OAuth(error)) => {
            return error.into_http_response().map(Body::from);
        }
        Err(OAuthAuthenticationError::Repository(_)) => return internal_error(),
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let scopes = authenticated.scopes().clone();
    let only_media = streaming_query_parameter(query.as_deref(), "only_media")
        .is_some_and(|value| activitypub_truthy(&value));
    let initial_stream = streaming_query_parameter(query.as_deref(), "stream")
        .or_else(|| streaming_path_stream(uri.path(), only_media).map(str::to_owned));
    let initial_command = streaming_initial_command(initial_stream, query.as_deref());
    let websocket = if let Some(protocol) = websocket_protocol {
        websocket.protocols([protocol])
    } else {
        websocket
    };
    websocket.on_upgrade(move |socket| {
        streaming_connection(
            socket,
            state,
            queue,
            auth_headers,
            owner.user_id(),
            owner.account_id(),
            authenticated.token_id(),
            scopes,
            cursor,
            initial_command,
        )
    })
}

pub(super) fn streaming_credentials(
    headers: &HeaderMap,
    query: Option<&str>,
) -> (HeaderMap, Option<String>) {
    if headers.contains_key(AUTHORIZATION) {
        return (headers.clone(), None);
    }
    let query_token =
        streaming_query_parameter(query, "access_token").filter(|token| !token.is_empty());
    let protocol = if query_token.is_none() {
        headers
            .get(SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    } else {
        None
    };
    let Some(token) = query_token.or_else(|| protocol.clone()) else {
        return (headers.clone(), None);
    };
    let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
        return (headers.clone(), None);
    };
    let mut headers = headers.clone();
    headers.insert(AUTHORIZATION, value);
    (headers, protocol)
}

pub(super) fn streaming_query_parameter(query: Option<&str>, name: &str) -> Option<String> {
    parameters(query)
        .into_iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

pub(super) fn streaming_path_stream(path: &str, only_media: bool) -> Option<&'static str> {
    match path {
        "/api/v1/streaming/user" => Some("user"),
        "/api/v1/streaming/user/notification" => Some("user:notification"),
        "/api/v1/streaming/direct" => Some("direct"),
        "/api/v1/streaming/public" => Some(if only_media { "public:media" } else { "public" }),
        "/api/v1/streaming/public/local" => Some(if only_media {
            "public:local:media"
        } else {
            "public:local"
        }),
        "/api/v1/streaming/public/remote" => Some(if only_media {
            "public:remote:media"
        } else {
            "public:remote"
        }),
        "/api/v1/streaming/hashtag" => Some("hashtag"),
        "/api/v1/streaming/hashtag/local" => Some("hashtag:local"),
        "/api/v1/streaming/list" => Some("list"),
        _ => None,
    }
}

pub(super) fn streaming_initial_command(
    stream: Option<String>,
    query: Option<&str>,
) -> ParsedCommand {
    let Some(stream) = stream else {
        return ParsedCommand::Ignore;
    };
    let mut command = serde_json::Map::from_iter([
        (
            "type".to_owned(),
            serde_json::Value::String("subscribe".to_owned()),
        ),
        ("stream".to_owned(), serde_json::Value::String(stream)),
    ]);
    for name in ["tag", "list"] {
        if let Some(value) = streaming_query_parameter(query, name) {
            command.insert(name.to_owned(), serde_json::Value::String(value));
        }
    }
    ClientCommand::parse(&serde_json::Value::Object(command).to_string())
}

#[derive(Default)]
pub(super) struct StreamingSubscriptions {
    pub(super) values: BTreeMap<Subscription, i64>,
}

impl StreamingSubscriptions {
    pub(super) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(super) fn has_capacity_for(&self, subscription: &Subscription) -> bool {
        self.values.contains_key(subscription) || self.values.len() < STREAM_MAX_SUBSCRIPTIONS
    }

    pub(super) fn insert(&mut self, subscription: Subscription, baseline_cursor: i64) {
        self.values.insert(subscription, baseline_cursor);
    }

    pub(super) fn remove(&mut self, subscription: &Subscription) {
        self.values.remove(subscription);
    }

    pub(super) fn contains(&self, stream: StreamName) -> bool {
        self.values.contains_key(&Subscription::from(stream))
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn streaming_connection(
    mut socket: WebSocket,
    state: WebState,
    queue: Queue,
    auth_headers: HeaderMap,
    user_id: i64,
    account_id: i64,
    token_id: i64,
    scopes: OAuthScopes,
    mut cursor: i64,
    initial_command: ParsedCommand,
) {
    let live_cursor = cursor;
    let mut subscriptions = StreamingSubscriptions::default();
    let mut has_established_subscription = false;
    let mut system_cursor = cursor;
    let mut heartbeat = interval(StdDuration::from_secs(30));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let mut poll = interval(StdDuration::from_millis(100));
    poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    poll.tick().await;
    let mut last_pong = Instant::now();

    if !streaming_handle_command(
        &mut socket,
        &state,
        &queue,
        &mut subscriptions,
        &mut has_established_subscription,
        initial_command,
        user_id,
        account_id,
        live_cursor,
        &scopes,
    )
    .await
    {
        return;
    }

    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(message) = message else {
                    return;
                };
                let Ok(message) = message else {
                    return;
                };
                match message {
                    Message::Text(text) => {
                        let command = ClientCommand::parse(text.as_str());
                        if !streaming_handle_command(
                            &mut socket,
                            &state,
                            &queue,
                            &mut subscriptions,
                            &mut has_established_subscription,
                            command,
                            user_id,
                            account_id,
                            live_cursor,
                            &scopes,
                        ).await {
                            return;
                        }
                    }
                    Message::Binary(_) => {
                        let _ = socket
                            .send(Message::Close(Some(CloseFrame {
                                code: 1003,
                                reason: "The mastodon streaming server does not support binary messages".into(),
                            })))
                            .await;
                        return;
                    }
                    Message::Pong(_) => last_pong = Instant::now(),
                    Message::Close(_) => return,
                    Message::Ping(_) => {}
                }
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= StdDuration::from_mins(1) {
                    return;
                }
                let valid = match state.authenticator.authenticate(&auth_headers, NO_SCOPE).await {
                    Ok(authenticated) => authenticated
                        .require_user()
                        .is_ok_and(|owner| owner.account_id() == account_id),
                    Err(_) => false,
                };
                if !valid {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: 1000,
                            reason: "Invalid access token".into(),
                        })))
                        .await;
                    return;
                }
                if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    return;
                }
            }
            _ = poll.tick() => {
                let Ok(mut kill) =
                    stream_system_events(&queue, account_id, token_id, &mut system_cursor).await
                else {
                    return;
                };
                if !kill && !subscriptions.is_empty() {
                    match stream_pending_events(
                        &mut socket,
                        &state,
                        &queue,
                        user_id,
                        account_id,
                        token_id,
                        &scopes,
                        &mut subscriptions,
                        &mut cursor,
                        live_cursor,
                    )
                    .await
                    {
                        Ok(pending_kill) => kill = pending_kill,
                        Err(()) => return,
                    }
                }
                if kill {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: 1000,
                            reason: "Invalid access token".into(),
                        })))
                        .await;
                    return;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn streaming_handle_command(
    socket: &mut WebSocket,
    state: &WebState,
    queue: &Queue,
    subscriptions: &mut StreamingSubscriptions,
    has_established_subscription: &mut bool,
    command: ParsedCommand,
    user_id: i64,
    account_id: i64,
    connection_cursor: i64,
    scopes: &OAuthScopes,
) -> bool {
    match command {
        ParsedCommand::Command(ClientCommand::Subscribe(subscription)) => {
            streaming_subscribe(
                socket,
                state,
                queue,
                subscriptions,
                has_established_subscription,
                subscription,
                user_id,
                account_id,
                connection_cursor,
                scopes,
            )
            .await
        }
        ParsedCommand::Command(ClientCommand::Unsubscribe(subscription)) => {
            subscriptions.remove(&subscription);
            true
        }
        ParsedCommand::Reject(error) => match error.status {
            Some(status) => send_stream_error(socket, status, error.message).await,
            None => socket
                .send(Message::Text(
                    r#"{"error":"Error unsubscribing from channel"}"#.into(),
                ))
                .await
                .is_ok(),
        },
        ParsedCommand::Ignore => true,
    }
}

pub(super) fn subscription_create_after(
    connection_cursor: i64,
    subscribe_cursor: i64,
    has_established_subscription: bool,
) -> i64 {
    if has_established_subscription {
        subscribe_cursor
    } else {
        connection_cursor
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn streaming_subscribe(
    socket: &mut WebSocket,
    state: &WebState,
    queue: &Queue,
    subscriptions: &mut StreamingSubscriptions,
    has_established_subscription: &mut bool,
    subscription: Subscription,
    user_id: i64,
    account_id: i64,
    connection_cursor: i64,
    scopes: &OAuthScopes,
) -> bool {
    let stream = subscription.stream();
    if !subscriptions.has_capacity_for(&subscription) {
        return send_stream_error(socket, 400, "Too many subscriptions").await;
    }
    if subscriptions.values.contains_key(&subscription) {
        return true;
    }
    let subscribe_cursor = if subscription.is_timeline() {
        let Ok(cursor) = queue.stream_cursor().await else {
            return false;
        };
        cursor
    } else {
        0
    };
    let create_after = subscription_create_after(
        connection_cursor,
        subscribe_cursor,
        *has_established_subscription,
    );
    if !stream.permits(scopes) {
        return send_stream_error(
            socket,
            401,
            "Access token does not have the required scopes",
        )
        .await;
    }
    let authorization = if stream == StreamName::List {
        let list_id = subscription
            .parameter()
            .and_then(|list| list.parse::<i64>().ok())
            .filter(|id| *id > 0);
        match list_id {
            Some(list_id) => state
                .repository
                .rest_owned_list_exists(account_id, list_id)
                .await
                .map_err(|_| ()),
            None => Ok(false),
        }
    } else if matches!(stream, StreamName::Hashtag | StreamName::HashtagLocal)
        && subscription.parameter().is_none_or(str::is_empty)
    {
        Ok(false)
    } else if stream.is_timeline() {
        stream_timeline_options(state, user_id, account_id, stream)
            .await
            .map(|options| options.is_some())
    } else {
        Ok(true)
    };
    match authorization {
        Ok(true) => {}
        Ok(false) => {
            let (status, message) = if stream == StreamName::List {
                (401, "Not authorized to stream this list")
            } else if matches!(stream, StreamName::Hashtag | StreamName::HashtagLocal) {
                (400, "Missing tag name parameter")
            } else {
                (401, "Not authorized to stream this feed")
            };
            return send_stream_error(socket, status, message).await;
        }
        Err(()) => return false,
    }
    let replay_through = if subscription.is_timeline() {
        let Ok(cursor) = queue.stream_cursor().await else {
            return false;
        };
        cursor
    } else {
        0
    };
    if subscription.is_timeline() {
        let Ok(events) = queue
            .stream_replay_events_for_subscription(
                replay_through,
                create_after,
                &subscription,
                account_id,
            )
            .await
        else {
            return false;
        };
        for event in events {
            if stream_timeline_event_for_subscription(
                socket,
                state,
                user_id,
                account_id,
                &subscription,
                &event,
                Some(create_after),
            )
            .await
            .is_err()
            {
                return false;
            }
        }
    }
    subscriptions.insert(subscription, replay_through);
    *has_established_subscription = true;
    true
}

pub(super) async fn send_stream_error(socket: &mut WebSocket, status: u16, error: &str) -> bool {
    let message = format!(
        "{{\"error\":{},\"status\":{status}}}",
        serde_json::to_string(error).expect("stream error is serializable")
    );
    socket.send(Message::Text(message.into())).await.is_ok()
}

pub(super) async fn stream_timeline_options(
    state: &WebState,
    user_id: i64,
    account_id: i64,
    stream: StreamName,
) -> Result<Option<TimelineOptions>, ()> {
    let mut options = TimelineOptions {
        local: matches!(
            stream,
            StreamName::PublicLocal | StreamName::PublicLocalMedia | StreamName::HashtagLocal
        ),
        remote: matches!(
            stream,
            StreamName::PublicRemote | StreamName::PublicRemoteMedia
        ),
        only_media: matches!(
            stream,
            StreamName::PublicMedia | StreamName::PublicLocalMedia | StreamName::PublicRemoteMedia
        ),
        ..TimelineOptions::default()
    };
    let settings = state.repository.settings().await.map_err(|_| ())?;
    let setting = |name: &str| {
        settings
            .iter()
            .find(|setting| setting.var == name)
            .and_then(|setting| setting.value.as_ref())
            .and_then(|value| yaml_scalar(value.raw()))
            .unwrap_or_else(|| "public".to_owned())
    };
    let topic = matches!(stream, StreamName::Hashtag | StreamName::HashtagLocal);
    let (local, remote) = if topic {
        (
            setting("local_topic_feed_access"),
            setting("remote_topic_feed_access"),
        )
    } else {
        (
            setting("local_live_feed_access"),
            setting("remote_live_feed_access"),
        )
    };
    let can_view_disabled = state
        .repository
        .user_can_view_feeds(user_id, account_id)
        .await
        .map_err(|_| ())?;
    let allowed = |setting: &str| match setting {
        "public" | "authenticated" => true,
        "disabled" => can_view_disabled,
        _ => false,
    };
    let access = FeedAccess {
        local: allowed(&local),
        remote: allowed(&remote),
    };
    Ok(apply_feed_access(&mut options, access).then_some(options))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn stream_pending_events(
    socket: &mut WebSocket,
    state: &WebState,
    queue: &Queue,
    user_id: i64,
    account_id: i64,
    token_id: i64,
    scopes: &OAuthScopes,
    subscriptions: &mut StreamingSubscriptions,
    cursor: &mut i64,
    live_cursor: i64,
) -> Result<bool, ()> {
    let events = queue
        .stream_events_after(*cursor, STREAM_EVENT_BATCH_SIZE)
        .await
        .map_err(|_| ())?;
    for event in events {
        *cursor = event.id;
        if event.account_id == 0 {
            stream_timeline_event(socket, state, user_id, account_id, subscriptions, &event)
                .await?;
            continue;
        }
        if event.id <= live_cursor || event.account_id != account_id {
            continue;
        }
        if event.event == SYSTEM_KILL_EVENT
            || (event.event == TOKEN_KILL_EVENT && event.object_id == token_id)
        {
            return Ok(true);
        }
        if event.event == TOKEN_KILL_EVENT {
            continue;
        }
        let streams = stream_account_targets(&event, scopes, subscriptions);
        if streams.is_empty() {
            continue;
        }
        let Some(payload) = stream_event_payload(state, account_id, &event).await? else {
            continue;
        };
        for subscription in streams {
            socket
                .send(Message::Text(
                    event_message(&subscription, stream_protocol_event(&event.event), &payload)
                        .into(),
                ))
                .await
                .map_err(|_| ())?;
        }
    }
    Ok(false)
}

pub(super) async fn stream_timeline_event(
    socket: &mut WebSocket,
    state: &WebState,
    user_id: i64,
    account_id: i64,
    subscriptions: &StreamingSubscriptions,
    event: &StreamEvent,
) -> Result<(), ()> {
    let timeline_subscriptions = subscriptions
        .values
        .iter()
        .filter(|(subscription, baseline_cursor)| {
            subscription.is_timeline() && event.id > **baseline_cursor
        })
        .map(|(subscription, _)| subscription.clone())
        .collect::<Vec<_>>();
    for subscription in timeline_subscriptions {
        stream_timeline_event_for_subscription(
            socket,
            state,
            user_id,
            account_id,
            &subscription,
            event,
            None,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn stream_timeline_event_for_subscription(
    socket: &mut WebSocket,
    state: &WebState,
    user_id: i64,
    account_id: i64,
    subscription: &Subscription,
    event: &StreamEvent,
    create_after: Option<i64>,
) -> Result<(), ()> {
    if !matches!(event.event.as_str(), "update" | "status.update" | "delete") {
        return Ok(());
    }
    let before = match event.before.as_ref() {
        Some(snapshot) => {
            // The before snapshot is authoritative for route membership: the current status row
            // already reflects the after state (and may already be suspended or silenced).
            stream_timeline_delete_snapshot_contains(
                state,
                user_id,
                account_id,
                subscription,
                snapshot,
            )
            .await?
        }
        None => false,
    };
    let after = match event.after.as_ref() {
        Some(snapshot) => {
            stream_timeline_snapshot_contains(
                state,
                user_id,
                account_id,
                subscription,
                event.object_id,
                snapshot,
            )
            .await?
        }
        None => false,
    };
    let protocol_event = match create_after {
        Some(create_after) => {
            timeline_replay_protocol_event(&event.event, before, after, event.id, create_after)
        }
        None => timeline_protocol_event(&event.event, before, after),
    };
    let Some(protocol_event) = protocol_event else {
        return Ok(());
    };
    let payload = if protocol_event == "delete" {
        event.object_id.to_string()
    } else {
        let Some(payload) = stream_event_payload(state, account_id, event).await? else {
            return Ok(());
        };
        payload
    };
    socket
        .send(Message::Text(
            event_message(subscription, protocol_event, &payload).into(),
        ))
        .await
        .map_err(|_| ())
}

pub(super) fn timeline_protocol_event(
    event: &str,
    before: bool,
    after: bool,
) -> Option<&'static str> {
    match (before, after) {
        (false, true) => Some("update"),
        (true, true) => Some("status.update"),
        (true, false) if event != "delete" => Some("status.update"),
        (true, false) => Some("delete"),
        (false, false) => None,
    }
}

pub(super) fn timeline_replay_protocol_event(
    event: &str,
    before: bool,
    after: bool,
    event_id: i64,
    create_after: i64,
) -> Option<&'static str> {
    timeline_protocol_event(event, before, after)
        .filter(|protocol_event| *protocol_event != "update" || event_id > create_after)
}

pub(super) async fn stream_timeline_delete_snapshot_contains(
    state: &WebState,
    user_id: i64,
    account_id: i64,
    subscription: &Subscription,
    snapshot: &TimelineRouteSnapshot,
) -> Result<bool, ()> {
    let stream = subscription.stream();
    if stream != StreamName::List
        && stream_timeline_options(state, user_id, account_id, stream)
            .await?
            .is_none()
    {
        return Ok(false);
    }
    let language_allowed = if matches!(
        stream,
        StreamName::Public
            | StreamName::PublicMedia
            | StreamName::PublicLocal
            | StreamName::PublicLocalMedia
            | StreamName::PublicRemote
            | StreamName::PublicRemoteMedia
    ) {
        state
            .repository
            .stream_public_language_allowed(account_id, snapshot.language.as_deref())
            .await
            .map_err(|_| ())?
    } else {
        true
    };
    Ok(timeline_delete_snapshot_matches(
        snapshot,
        subscription,
        account_id,
        language_allowed,
    ))
}

pub(super) fn timeline_delete_snapshot_matches(
    snapshot: &TimelineRouteSnapshot,
    subscription: &Subscription,
    account_id: i64,
    language_allowed: bool,
) -> bool {
    let stream = subscription.stream();
    match stream {
        StreamName::Public
        | StreamName::PublicMedia
        | StreamName::PublicLocal
        | StreamName::PublicLocalMedia
        | StreamName::PublicRemote
        | StreamName::PublicRemoteMedia => {
            let locality_matches = match stream {
                StreamName::PublicLocal | StreamName::PublicLocalMedia => snapshot.local,
                StreamName::PublicRemote | StreamName::PublicRemoteMedia => !snapshot.local,
                _ => true,
            };
            let media_matches = !matches!(
                stream,
                StreamName::PublicMedia
                    | StreamName::PublicLocalMedia
                    | StreamName::PublicRemoteMedia
            ) || snapshot.had_media;
            snapshot.public && locality_matches && media_matches && language_allowed
        }
        StreamName::Hashtag | StreamName::HashtagLocal => {
            snapshot.hashtag
                && (stream != StreamName::HashtagLocal || snapshot.local)
                && subscription
                    .parameter()
                    .is_some_and(|tag| snapshot.tags.iter().any(|candidate| candidate == tag))
        }
        StreamName::List => subscription
            .parameter()
            .and_then(|list| list.parse::<i64>().ok())
            .is_some_and(|list_id| {
                snapshot
                    .lists
                    .iter()
                    .any(|route| route.account_id == account_id && route.list_id == list_id)
            }),
        StreamName::User | StreamName::UserNotification | StreamName::Direct => false,
    }
}

pub(super) async fn stream_timeline_snapshot_contains(
    state: &WebState,
    user_id: i64,
    account_id: i64,
    subscription: &Subscription,
    status_id: i64,
    snapshot: &TimelineRouteSnapshot,
) -> Result<bool, ()> {
    let stream = subscription.stream();
    match stream {
        StreamName::Public
        | StreamName::PublicMedia
        | StreamName::PublicLocal
        | StreamName::PublicLocalMedia
        | StreamName::PublicRemote
        | StreamName::PublicRemoteMedia => {
            let Some(options) = stream_timeline_options(state, user_id, account_id, stream).await?
            else {
                return Ok(false);
            };
            state
                .repository
                .stream_public_timeline_contains(
                    account_id,
                    status_id,
                    options,
                    true,
                    Some(snapshot.had_media),
                    snapshot.language.as_deref(),
                    true,
                )
                .await
                .map_err(|_| ())
        }
        StreamName::Hashtag | StreamName::HashtagLocal => {
            let Some(tag) = subscription.parameter() else {
                return Ok(false);
            };
            let Some(options) = stream_timeline_options(state, user_id, account_id, stream).await?
            else {
                return Ok(false);
            };
            let tag_matches = snapshot.tags.iter().any(|candidate| candidate == tag);
            state
                .repository
                .stream_tag_timeline_contains(
                    account_id,
                    status_id,
                    tag,
                    options,
                    true,
                    Some(tag_matches),
                    Some(snapshot.had_media),
                )
                .await
                .map_err(|_| ())
        }
        StreamName::List => {
            let Some(list_id) = subscription
                .parameter()
                .and_then(|list| list.parse::<i64>().ok())
            else {
                return Ok(false);
            };
            let historically_matched = snapshot
                .lists
                .iter()
                .any(|route| route.account_id == account_id && route.list_id == list_id);
            if !historically_matched {
                return Ok(false);
            }
            state
                .repository
                .stream_list_timeline_contains(
                    account_id,
                    list_id,
                    status_id,
                    true,
                    snapshot.language.as_deref(),
                )
                .await
                .map(|current| current.unwrap_or(true))
                .map_err(|_| ())
        }
        StreamName::User | StreamName::UserNotification | StreamName::Direct => Ok(false),
    }
}

pub(super) async fn stream_system_events(
    queue: &Queue,
    account_id: i64,
    token_id: i64,
    cursor: &mut i64,
) -> Result<bool, ()> {
    let events = queue
        .stream_events_after(*cursor, STREAM_EVENT_BATCH_SIZE)
        .await
        .map_err(|_| ())?;
    for event in events {
        *cursor = event.id;
        if event.account_id == account_id
            && (event.event == SYSTEM_KILL_EVENT
                || (event.event == TOKEN_KILL_EVENT && event.object_id == token_id))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn stream_account_targets(
    event: &StreamEvent,
    scopes: &OAuthScopes,
    subscriptions: &StreamingSubscriptions,
) -> Vec<Subscription> {
    if event.event == STATUS_UPDATE_NOTIFICATION_EVENT {
        let mut streams = Vec::with_capacity(2);
        if subscriptions.contains(StreamName::User)
            && StreamName::User.includes_notifications(scopes)
        {
            streams.push(StreamName::User.into());
        }
        if subscriptions.contains(StreamName::UserNotification)
            && StreamName::UserNotification.permits(scopes)
        {
            streams.push(StreamName::UserNotification.into());
        }
        streams
    } else if event.event == "conversation" {
        if subscriptions.contains(StreamName::Direct) && StreamName::Direct.permits(scopes) {
            vec![StreamName::Direct.into()]
        } else {
            Vec::new()
        }
    } else if matches!(
        event.event.as_str(),
        "notification" | "notifications_merged"
    ) {
        let mut streams = Vec::with_capacity(2);
        if subscriptions.contains(StreamName::User)
            && StreamName::User.includes_notifications(scopes)
        {
            streams.push(StreamName::User.into());
        }
        if subscriptions.contains(StreamName::UserNotification)
            && StreamName::UserNotification.permits(scopes)
        {
            streams.push(StreamName::UserNotification.into());
        }
        streams
    } else if subscriptions.contains(StreamName::User) {
        vec![StreamName::User.into()]
    } else {
        Vec::new()
    }
}

pub(super) fn stream_protocol_event(event: &str) -> &str {
    if event == STATUS_UPDATE_NOTIFICATION_EVENT {
        "status.update"
    } else {
        event
    }
}

pub(super) async fn stream_event_payload(
    state: &WebState,
    account_id: i64,
    event: &StreamEvent,
) -> Result<Option<String>, ()> {
    match event.event.as_str() {
        "delete" => Ok(Some(event.object_id.to_string())),
        "update" | "status.update" | STATUS_UPDATE_NOTIFICATION_EVENT => {
            let Some(status) = state
                .loader(Some(account_id))
                .authorized_status(event.object_id)
                .await
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            serde_json::to_string(
                &state
                    .serializer()
                    .status(&status, StatusShape::Full)
                    .map_err(|_| ())?,
            )
            .map(Some)
            .map_err(|_| ())
        }
        "notification" => {
            let Some(notification) = state
                .loader(Some(account_id))
                .notification(account_id, event.object_id)
                .await
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            serde_json::to_string(
                &state
                    .serializer()
                    .notification(&notification, None)
                    .map_err(|_| ())?,
            )
            .map(Some)
            .map_err(|_| ())
        }
        "notifications_merged" => Ok(Some("1".to_owned())),
        "conversation" => {
            let Some(conversation) = state
                .loader(Some(account_id))
                .conversation(account_id, event.object_id)
                .await
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            serde_json::to_string(&serialize_conversation(state, &conversation).map_err(|_| ())?)
                .map(Some)
                .map_err(|_| ())
        }
        _ => Ok(None),
    }
}
