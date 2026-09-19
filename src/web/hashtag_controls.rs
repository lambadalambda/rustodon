//! Hashtag controls are local API/database operations; AP projections are deferred.
use super::{
    Body, Extension, HeaderMap, NO_SCOPE, RackParameters, RackValue, Response, State, StatusCode,
    StdDuration, Uri, WRITE_ACCOUNTS, WRITE_FOLLOWS, WebState, error_response, internal_error,
    json_response, optional_viewer, rate_limited_response, record_not_found, required_write_viewer,
    status_saved_write_error, uri_path_id, uri_path_segment,
};
use crate::mastodon::rest::{TagHistoryProjection, TagProjection};
use crate::mastodon::{
    hashtag::{display_name, valid_name},
    normalize_hashtag,
};

async fn tag_response(state: &WebState, name: &str, viewer: Option<i64>) -> Response<Body> {
    let Ok(tag) = state.loader(viewer).tag(name).await else {
        return internal_error();
    };
    let persisted = tag.is_some();
    let tag = tag.unwrap_or_else(|| {
        let today = chrono::Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        TagProjection {
            id: 0,
            name: normalize_hashtag(name),
            display_name: Some(display_name(name)),
            history: (0..7)
                .map(|day| TagHistoryProjection {
                    day: (today - day * 86400).to_string(),
                    uses: "0".to_owned(),
                    accounts: "0".to_owned(),
                })
                .collect(),
            following: viewer.map(|_| false),
            featuring: viewer.map(|_| false),
        }
    });
    let mut value = state.serializer().tag(&tag);
    if !persisted {
        value.id.clear();
    }
    match serde_json::to_vec(&value) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

pub(super) async fn show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, NO_SCOPE).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some(name) = uri_path_segment(&uri, 4).filter(|name| valid_name(name)) else {
        return record_not_found();
    };
    tag_response(&state, &name, viewer).await
}

pub(super) async fn mutate(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let action = uri_path_segment(&uri, 5).unwrap_or_default();
    let featured = matches!(action.as_str(), "feature" | "unfeature");
    let enabled = matches!(action.as_str(), "feature" | "follow");
    let auth = match required_write_viewer(
        &state,
        &headers,
        if featured {
            WRITE_ACCOUNTS
        } else {
            WRITE_FOLLOWS
        },
    )
    .await
    {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    let owner = auth.resource_owner().expect("required user").account_id();
    let Some(name) = uri_path_segment(&uri, 4).filter(|name| valid_name(name)) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if !featured && enabled {
        let Ok(following) = state.repository.follows_tag(owner, &name).await else {
            return internal_error();
        };
        if !following {
            let Some(limiter) = state.shared_rate_limiter.as_ref() else {
                return internal_error();
            };
            if let Err(limited) = limiter
                .try_allow([(format!("follows:{owner}"), 400, StdDuration::from_hours(24))])
                .await
            {
                return rate_limited_response(limited);
            }
        }
    }
    if let Err(error) = writer
        .set_tag_relationship(&auth, &name, featured, enabled, false)
        .await
    {
        return status_saved_write_error(&error);
    }
    // As with other write/readback handlers, the relationship is committed before
    // projection. A history failure returns 500, never fabricated zero history;
    // the persisted idempotent relationship is safe for the client to retry.
    tag_response(&state, &name, Some(owner)).await
}

pub(super) async fn create_featured(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let auth = match required_write_viewer(&state, &headers, WRITE_ACCOUNTS).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    let Some(RackValue::Scalar(name)) = rack.get("name") else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "param is missing or the value is empty: name",
        );
    };
    if name.trim().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "param is missing or the value is empty: name",
        );
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .set_tag_relationship(&auth, name, true, true, true)
        .await
    {
        return status_saved_write_error(&error);
    }
    let owner = auth.resource_owner().expect("required user").account_id();
    let Ok(tags) = state.loader(Some(owner)).featured_tags(owner).await else {
        return internal_error();
    };
    let normalized = normalize_hashtag(name);
    let Some(tag) = tags.iter().find(|tag| tag.tag_name == normalized) else {
        return internal_error();
    };
    match serde_json::to_vec(&state.serializer().featured_tag(tag)) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

pub(super) async fn delete_featured(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let auth = match required_write_viewer(&state, &headers, WRITE_ACCOUNTS).await {
        Ok(auth) => auth,
        Err(response) => return response,
    };
    let Some((_, id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer.remove_featured_tag(&auth, id).await {
        Ok(()) => json_response(StatusCode::OK, b"{}".to_vec()),
        Err(error) => status_saved_write_error(&error),
    }
}
