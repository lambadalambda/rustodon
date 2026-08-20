use std::collections::VecDeque;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::SystemTime;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Extension, Path, Query, RawQuery, Request, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_HEADERS,
    ACCESS_CONTROL_REQUEST_METHOD, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, HOST, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, LOCATION, ORIGIN, RANGE,
    VARY,
};
use axum::http::{HeaderMap, HeaderValue, Method, Response, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::routing::{any, get};
use chrono::Utc;
use futures_util::TryStreamExt;
use ipnetwork::IpNetwork;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use url::Url;

use crate::mastodon::rest::{
    AccountListKind, AccountListOptions, AccountSearchError, AccountStatusesOptions,
    FollowCollectionKind, FollowCollectionOptions, InstanceRuntimeConfig, RestProjectionLoader,
    RestSerializer, SavedStatusKind, SavedStatusesOptions, StatusShape, TagTimelineOptions,
    TimelineOptions,
};
use crate::mastodon::{
    Account, BearerAuthenticator, NO_SCOPE, OAuthAuthenticationError, OAuthError,
    OAuthResourceOwner, READ_ACCOUNTS, READ_BLOCKS, READ_BOOKMARKS, READ_COLLECTIONS,
    READ_FAVOURITES, READ_FILTERS, READ_FOLLOWS, READ_LISTS, READ_MUTES, READ_STATUSES, Repository,
    RequiredScopes, VERIFY_CREDENTIALS,
    activitypub::{self, ACTIVITY_JSON, JRD_JSON},
};
use crate::paperclip::{PaperclipRoot, parse_paperclip_path};

const CORS_METHODS: &str = "POST, PUT, DELETE, GET, PATCH, OPTIONS";
const CORS_MAX_AGE: &str = "7200";
const CORS_EXPOSE_HEADERS: &str = "Link, Mastodon-Async-Refresh, X-RateLimit-Reset, X-RateLimit-Limit, X-RateLimit-Remaining, X-Request-Id";
const PUBLIC_CACHE: &str = "max-age=300, public, stale-while-revalidate=30, stale-if-error=86400";
const ANONYMOUS_CACHE: &str = "max-age=15, public, stale-while-revalidate=30, stale-if-error=86400";
const PRIVATE_CACHE: &str = "private, no-store";
const PAPERCLIP_CACHE: &str = "public, max-age=2419200, immutable";
const PAPERCLIP_CSP: &str = "default-src 'none'; form-action 'none'";
const MULTIPART_BOUNDARY: &str = "AaB03x";
const FRAMEWORK_ERROR_HEADER: &str = "x-rustodon-framework-error";
const RACK_BYTES_LIMIT: usize = 4 * 1024 * 1024;
const RACK_PARAMETER_LIMIT: usize = 4096;
const RACK_DEPTH_LIMIT: usize = 32;
pub const REST_BODY_LIMIT_BYTES: usize = 99 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestMetadata {
    pub client_ip: IpAddr,
    pub scheme: Option<String>,
    pub host: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForwardedHeaderError;

/// Resolves request metadata from trusted forwarding headers or the direct socket peer.
///
/// # Errors
///
/// Rejects malformed forwarding values supplied by a configured trusted proxy.
pub fn request_metadata(
    peer: SocketAddr,
    headers: &HeaderMap,
    trusted_proxies: &[IpNetwork],
) -> Result<RequestMetadata, ForwardedHeaderError> {
    if !trusted_proxies
        .iter()
        .any(|network| network.contains(peer.ip()))
    {
        return Ok(RequestMetadata {
            client_ip: peer.ip(),
            scheme: None,
            host: None,
        });
    }
    let forwarded_for = header_text(headers, "x-forwarded-for")?;
    let client_ip = forwarded_for.map_or(Ok(peer.ip()), |chain| {
        let addresses = chain
            .split(',')
            .map(str::trim)
            .map(str::parse::<IpAddr>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ForwardedHeaderError)?;
        if addresses.is_empty() {
            return Err(ForwardedHeaderError);
        }
        Ok(addresses
            .iter()
            .copied()
            .rev()
            .find(|address| {
                !trusted_proxies
                    .iter()
                    .any(|network| network.contains(*address))
            })
            .unwrap_or(addresses[0]))
    })?;
    let scheme = header_text(headers, "x-forwarded-proto")?
        .map(str::to_ascii_lowercase)
        .filter(|value| matches!(value.as_str(), "http" | "https"));
    if headers.contains_key("x-forwarded-proto") && scheme.is_none() {
        return Err(ForwardedHeaderError);
    }
    let host = header_text(headers, "x-forwarded-host")?.map(str::to_ascii_lowercase);
    Ok(RequestMetadata {
        client_ip,
        scheme,
        host,
    })
}

fn header_text<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<Option<&'a str>, ForwardedHeaderError> {
    headers
        .get(name)
        .map(|value| value.to_str().map_err(|_| ForwardedHeaderError))
        .transpose()
}
const RAILS_PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b'-')
    .remove(b'.')
    .remove(b':')
    .remove(b';')
    .remove(b'=')
    .remove(b'@')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiRouteSupport {
    Implemented,
    DisabledResponse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiMethod {
    Get,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiAuthentication {
    Public,
    Optional(&'static [&'static str]),
    Required(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaginationContract {
    None,
    StatusId,
    RelationshipId,
    AssociationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApiCachePolicy {
    Public,
    Anonymous,
    Private,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiRouteContract {
    pub path: &'static str,
    pub method: ApiMethod,
    pub support: ApiRouteSupport,
    pub authentication: ApiAuthentication,
    pub pagination: PaginationContract,
    cache: ApiCachePolicy,
}

macro_rules! route {
    ($path:literal, $support:ident, $authentication:expr, $pagination:ident, $cache:ident) => {
        ApiRouteContract {
            path: $path,
            method: ApiMethod::Get,
            support: ApiRouteSupport::$support,
            authentication: $authentication,
            pagination: PaginationContract::$pagination,
            cache: ApiCachePolicy::$cache,
        }
    };
}

pub const API_ROUTE_INVENTORY: &[ApiRouteContract] = &[
    route!(
        "/api/v1/instance",
        Implemented,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v2/instance",
        Implemented,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v1/instance/rules",
        Implemented,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v1/instance/translation_languages",
        DisabledResponse,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v1/accounts/lookup",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/search",
        Implemented,
        ApiAuthentication::Required(READ_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/markers",
        Implemented,
        ApiAuthentication::Required(READ_STATUSES.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v2/filters",
        Implemented,
        ApiAuthentication::Required(READ_FILTERS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/lists",
        Implemented,
        ApiAuthentication::Required(READ_LISTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/featured_tags",
        Implemented,
        ApiAuthentication::Required(READ_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/{id}/featured_tags",
        Implemented,
        ApiAuthentication::Public,
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/relationships",
        Implemented,
        ApiAuthentication::Required(READ_FOLLOWS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/verify_credentials",
        Implemented,
        ApiAuthentication::Required(VERIFY_CREDENTIALS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/{id}",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/collections/{id}",
        Implemented,
        ApiAuthentication::Optional(READ_COLLECTIONS.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/{id}/statuses",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        StatusId,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/{id}/followers",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        RelationshipId,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/{id}/following",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        RelationshipId,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}/source",
        Implemented,
        ApiAuthentication::Required(READ_STATUSES.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/statuses/{id}/history",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}/favourited_by",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        AssociationId,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}/reblogged_by",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        StatusId,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}/context",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/timelines/public",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        StatusId,
        Anonymous
    ),
    route!(
        "/api/v1/timelines/tag/{hashtag}",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        StatusId,
        Anonymous
    ),
    route!(
        "/api/v1/timelines/home",
        Implemented,
        ApiAuthentication::Required(READ_STATUSES.as_slice()),
        StatusId,
        Private
    ),
    route!(
        "/api/v1/timelines/list/{id}",
        Implemented,
        ApiAuthentication::Required(READ_LISTS.as_slice()),
        StatusId,
        Private
    ),
    route!(
        "/api/v1/favourites",
        Implemented,
        ApiAuthentication::Required(READ_FAVOURITES.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v1/bookmarks",
        Implemented,
        ApiAuthentication::Required(READ_BOOKMARKS.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v1/blocks",
        Implemented,
        ApiAuthentication::Required(READ_BLOCKS.as_slice()),
        RelationshipId,
        Private
    ),
    route!(
        "/api/v1/mutes",
        Implemented,
        ApiAuthentication::Required(READ_MUTES.as_slice()),
        RelationshipId,
        Private
    ),
];

#[derive(Clone)]
pub struct WebState {
    repository: Repository,
    authenticator: BearerAuthenticator,
    origin: Url,
    local_domain: String,
    media_root_url: String,
    media_root: PaperclipRoot,
    media_route_path: String,
    media_route_authority: Option<String>,
    instance_runtime: InstanceRuntimeConfig,
    trusted_proxies: Vec<IpNetwork>,
    allowed_hosts: Vec<String>,
}

impl WebState {
    /// Builds web state and securely opens the configured local media root.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the media root cannot be opened without following symlinks.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repository: Repository,
        origin: Url,
        local_domain: impl Into<String>,
        media_root_url: impl Into<String>,
        media_root_path: impl Into<PathBuf>,
        instance_runtime: InstanceRuntimeConfig,
        trusted_proxies: Vec<IpNetwork>,
        allowed_hosts: Vec<String>,
    ) -> std::io::Result<Self> {
        let media_root_url = media_root_url.into();
        let (media_route_path, media_route_authority) = media_route(&media_root_url);
        Ok(Self {
            authenticator: BearerAuthenticator::new(repository.clone()),
            repository,
            origin,
            local_domain: local_domain.into(),
            media_root_url,
            media_root: PaperclipRoot::open(&media_root_path.into())?,
            media_route_path,
            media_route_authority,
            instance_runtime,
            trusted_proxies,
            allowed_hosts,
        })
    }

    fn loader(&self, viewer_account_id: Option<i64>) -> RestProjectionLoader {
        RestProjectionLoader::new(
            self.repository.clone(),
            viewer_account_id,
            self.local_domain.clone(),
        )
    }

    fn serializer(&self) -> RestSerializer<'_> {
        RestSerializer::new(
            &self.origin,
            &self.local_domain,
            &self.media_root_url,
            Utc::now().naive_utc(),
        )
    }
}

fn media_route(media_root_url: &str) -> (String, Option<String>) {
    Url::parse(media_root_url).map_or_else(
        |_| (media_root_url.trim_end_matches('/').to_owned(), None),
        |url| {
            let authority = url.port().map_or_else(
                || url.host_str().unwrap_or_default().to_owned(),
                |port| format!("{}:{port}", url.host_str().unwrap_or_default()),
            );
            (url.path().trim_end_matches('/').to_owned(), Some(authority))
        },
    )
}

#[allow(clippy::too_many_lines)]
pub fn router(state: WebState) -> Router {
    let media_route = format!("{}/{{*path}}", state.media_route_path.trim_end_matches('/'));
    let federation = Router::new()
        .route("/.well-known/webfinger", get(federation_webfinger))
        .route("/.well-known/host-meta", get(federation_host_meta))
        .route("/.well-known/host-meta.json", get(federation_host_meta))
        .route("/.well-known/nodeinfo", get(federation_nodeinfo_discovery))
        .route("/nodeinfo/2.0", get(federation_nodeinfo))
        .route("/actor", get(federation_actor_instance))
        .route("/users/{username}", get(federation_actor_username))
        .route("/@{username}", get(federation_actor_username))
        .route("/ap/users/{id}", get(federation_actor_id))
        .route(
            "/users/{username}/statuses/{id}",
            get(federation_note_username),
        )
        .route(
            "/ap/users/{account_id}/statuses/{id}",
            get(federation_note_id),
        )
        .route("/users/{username}/outbox", get(federation_outbox_username))
        .route("/ap/users/{account_id}/outbox", get(federation_outbox_id))
        .route("/actor/outbox", get(federation_outbox_instance))
        .route(
            "/users/{username}/followers",
            get(federation_followers_username),
        )
        .route(
            "/users/{username}/following",
            get(federation_following_username),
        )
        .route(
            "/ap/users/{account_id}/followers",
            get(federation_followers_id),
        )
        .route(
            "/ap/users/{account_id}/following",
            get(federation_following_id),
        );
    let api = Router::new()
        .route("/health", get(health))
        .route("/ready", get(readiness))
        .route("/api/v1/instance", get(instance_v1))
        .route("/api/v2/instance", get(instance_v2))
        .route("/api/v1/instance/rules", get(instance_rules))
        .route(
            "/api/v1/instance/translation_languages",
            get(translation_languages),
        )
        .route("/api/v1/accounts/lookup", get(account_lookup))
        .route("/api/v1/accounts/search", get(account_search))
        .route("/api/v1/markers", get(markers))
        .route("/api/v2/filters", get(filters))
        .route("/api/v1/lists", get(lists))
        .route("/api/v1/featured_tags", get(featured_tags))
        .route(
            "/api/v1/accounts/{id}/featured_tags",
            get(account_featured_tags),
        )
        .route("/api/v1/accounts/relationships", get(relationships))
        .route(
            "/api/v1/accounts/verify_credentials",
            get(verify_credentials),
        )
        .route("/api/v1/accounts/{id}", get(account_show))
        .route("/api/v1/collections/{id}", get(collection_show))
        .route("/api/v1/accounts/{id}/statuses", get(account_statuses))
        .route("/api/v1/accounts/{id}/followers", get(account_followers))
        .route("/api/v1/accounts/{id}/following", get(account_following))
        .route("/api/v1/statuses/{id}", get(status_show))
        .route("/api/v1/statuses/{id}/source", get(status_source))
        .route("/api/v1/statuses/{id}/history", get(status_history))
        .route("/api/v1/statuses/{id}/favourited_by", get(favourited_by))
        .route("/api/v1/statuses/{id}/reblogged_by", get(reblogged_by))
        .route("/api/v1/statuses/{id}/context", get(status_context))
        .route("/api/v1/timelines/public", get(public_timeline))
        .route("/api/v1/timelines/tag/{hashtag}", get(tag_timeline))
        .route("/api/v1/timelines/home", get(home_timeline))
        .route("/api/v1/timelines/list/{id}", get(list_timeline))
        .route("/api/v1/favourites", get(favourites))
        .route("/api/v1/bookmarks", get(bookmarks))
        .route("/api/v1/blocks", get(blocks))
        .route("/api/v1/mutes", get(mutes))
        .route("/api/v1/instance/", get(instance_v1))
        .route("/api/v2/instance/", get(instance_v2))
        .route("/api/v1/instance/rules/", get(instance_rules))
        .route(
            "/api/v1/instance/translation_languages/",
            get(translation_languages),
        )
        .route("/api/v1/accounts/lookup/", get(account_lookup))
        .route("/api/v1/accounts/search/", get(account_search))
        .route("/api/v1/markers/", get(markers))
        .route("/api/v2/filters/", get(filters))
        .route("/api/v1/lists/", get(lists))
        .route("/api/v1/featured_tags/", get(featured_tags))
        .route(
            "/api/v1/accounts/{id}/featured_tags/",
            get(account_featured_tags),
        )
        .route("/api/v1/accounts/relationships/", get(relationships))
        .route(
            "/api/v1/accounts/verify_credentials/",
            get(verify_credentials),
        )
        .route("/api/v1/accounts/{id}/", get(account_show))
        .route("/api/v1/collections/{id}/", get(collection_show))
        .route("/api/v1/accounts/{id}/statuses/", get(account_statuses))
        .route("/api/v1/accounts/{id}/followers/", get(account_followers))
        .route("/api/v1/accounts/{id}/following/", get(account_following))
        .route("/api/v1/statuses/{id}/", get(status_show))
        .route("/api/v1/statuses/{id}/source/", get(status_source))
        .route("/api/v1/statuses/{id}/history/", get(status_history))
        .route("/api/v1/statuses/{id}/favourited_by/", get(favourited_by))
        .route("/api/v1/statuses/{id}/reblogged_by/", get(reblogged_by))
        .route("/api/v1/statuses/{id}/context/", get(status_context))
        .route("/api/v1/timelines/public/", get(public_timeline))
        .route("/api/v1/timelines/tag/{hashtag}/", get(tag_timeline))
        .route("/api/v1/timelines/home/", get(home_timeline))
        .route("/api/v1/timelines/list/{id}/", get(list_timeline))
        .route("/api/v1/favourites/", get(favourites))
        .route("/api/v1/bookmarks/", get(bookmarks))
        .route("/api/v1/blocks/", get(blocks))
        .route("/api/v1/mutes/", get(mutes))
        .fallback(|| async { api_not_found() })
        .method_not_allowed_fallback(|| async { api_not_found() })
        .layer(middleware::from_fn(api_protocol));
    Router::new()
        .route(&media_route, any(paperclip_media))
        .merge(federation)
        .merge(api)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_context,
        ))
        .with_state(state)
}

const FORWARDED_HEADERS: &[&str] = &[
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "x-real-ip",
    "client-ip",
];

async fn federation_account(
    state: &WebState,
    account_id: i64,
    reject_suspension: bool,
) -> Result<Account, Response<Body>> {
    let account = match state.repository.account(account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return Err(not_found()),
        Err(_) => return Err(internal_error()),
    };
    if account.id != -99
        && (account.domain.is_some() || account.has_pending_user || account.has_unconfirmed_user)
    {
        return Err(not_found());
    }
    if reject_suspension && account.suspended_at.is_some() {
        let Ok(temporary) = state
            .repository
            .account_has_deletion_request(account.id)
            .await
        else {
            return Err(internal_error());
        };
        return Err(error_response(
            if temporary {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::GONE
            },
            "Unavailable account",
        ));
    }
    Ok(account)
}

fn federation_domain_matches(state: &WebState, domain: &str) -> bool {
    let origin_authority = federation_url_authority(&state.origin);
    state
        .allowed_hosts
        .iter()
        .filter(|allowed| {
            state.media_route_authority.as_deref() != Some(allowed.as_str())
                || origin_authority.as_deref() == Some(allowed.as_str())
        })
        .any(|allowed| allowed.eq_ignore_ascii_case(domain))
}

fn federation_url_authority(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    Some(
        url.port()
            .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}")),
    )
}

async fn federation_local_account_id(
    state: &WebState,
    username: &str,
) -> Result<i64, Response<Body>> {
    match state
        .repository
        .rest_local_account_id_by_username(username)
        .await
    {
        Ok(Some(account_id)) => Ok(account_id),
        Ok(None) => Err(not_found()),
        Err(_) => Err(internal_error()),
    }
}

#[allow(clippy::too_many_lines)]
async fn federation_webfinger(
    State(state): State<WebState>,
    Query(parameters): Query<HashMap<String, String>>,
) -> Response<Body> {
    let Some(resource) = parameters.get("resource") else {
        return error_response(StatusCode::BAD_REQUEST, "Missing resource");
    };
    let account_id = if resource.starts_with("http://") || resource.starts_with("https://") {
        let Ok(url) = Url::parse(resource) else {
            return not_found();
        };
        let Some(authority) = federation_url_authority(&url) else {
            return not_found();
        };
        if !federation_domain_matches(&state, &authority) {
            return not_found();
        }
        let path = url.path().trim_end_matches('/');
        if path == "/actor" {
            -99
        } else if let Some(username) = path.strip_prefix("/@") {
            match state
                .repository
                .rest_local_account_id_by_username(username)
                .await
            {
                Ok(Some(account_id)) => account_id,
                Ok(None) => return not_found(),
                Err(_) => return internal_error(),
            }
        } else if let Some(username) = path.strip_prefix("/users/") {
            if username.is_empty() || username.contains('/') {
                return not_found();
            }
            match state
                .repository
                .rest_local_account_id_by_username(username)
                .await
            {
                Ok(Some(account_id)) => account_id,
                Ok(None) => return not_found(),
                Err(_) => return internal_error(),
            }
        } else if let Some(id) = path
            .strip_prefix("/ap/users/")
            .and_then(activitypub_path_id)
        {
            id
        } else {
            return not_found();
        }
    } else {
        let resource = resource.strip_prefix("acct:").unwrap_or(resource);
        let mut parts = resource.split('@');
        let Some(username) = parts.next() else {
            return error_response(StatusCode::BAD_REQUEST, "Invalid resource");
        };
        let Some(domain) = parts.next() else {
            return error_response(StatusCode::BAD_REQUEST, "Invalid resource");
        };
        if username.is_empty()
            || domain.is_empty()
            || parts.next().is_some()
            || !federation_domain_matches(&state, domain)
        {
            return not_found();
        }
        if username.eq_ignore_ascii_case(&state.local_domain)
            || username.eq_ignore_ascii_case(state.origin.host_str().unwrap_or_default())
        {
            -99
        } else {
            match state
                .repository
                .rest_local_account_id_by_username(username)
                .await
            {
                Ok(Some(account_id)) => account_id,
                Ok(None) => return not_found(),
                Err(_) => return internal_error(),
            }
        }
    };
    let account = match state.repository.account(account_id).await {
        Ok(Some(account)) if account.id == -99 || account.domain.is_none() => account,
        Ok(Some(_) | None) => return not_found(),
        Err(_) => return internal_error(),
    };
    if account.suspended_at.is_some() {
        let Ok(temporary) = state
            .repository
            .account_has_deletion_request(account.id)
            .await
        else {
            return internal_error();
        };
        if !temporary {
            return raw_response(StatusCode::GONE, "text/plain; charset=utf-8", Vec::new());
        }
    }
    activity_response(
        StatusCode::OK,
        JRD_JSON,
        activitypub::webfinger(
            &state.origin,
            &state.local_domain,
            &state.media_root_url,
            state.instance_runtime.limited_federation,
            &account,
        ),
    )
}

async fn federation_host_meta(
    State(state): State<WebState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let accept_json = uri.path().ends_with(".json")
        || headers
            .get("accept")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("application/json"));
    let (content_type, body) = activitypub::host_meta(&state.origin, accept_json);
    raw_response(StatusCode::OK, &content_type, body)
}

async fn federation_nodeinfo_discovery(State(state): State<WebState>) -> Response<Body> {
    activity_response(
        StatusCode::OK,
        "application/json; charset=utf-8",
        activitypub::nodeinfo_discovery(&state.origin),
    )
}

#[allow(clippy::manual_let_else)]
async fn federation_nodeinfo(State(state): State<WebState>) -> Response<Body> {
    let instance = match state
        .loader(None)
        .instance(state.instance_runtime.clone())
        .await
    {
        Ok(instance) => instance,
        Err(_) => return internal_error(),
    };
    activity_response(
        StatusCode::OK,
        "application/json; charset=utf-8",
        activitypub::nodeinfo(
            &instance.runtime.version,
            &instance.title,
            &instance.short_description,
            instance.user_count,
            instance.status_count,
            instance.runtime.active_month,
            instance.runtime.active_halfyear,
            instance.registrations_mode != "none" && !instance.runtime.single_user_mode,
        ),
    )
}

async fn federation_actor_instance(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    federation_actor_response(&state, -99, &headers).await
}

async fn federation_actor_username(
    State(state): State<WebState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !accepts_activitypub(&headers) {
        if uri.path().starts_with("/@") {
            return not_found();
        }
        return html_redirect(&state.origin, &format!("/@{username}"));
    }
    let account_id = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_actor_response(&state, account_id, &headers).await
}

async fn federation_actor_id(
    State(state): State<WebState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(id) = activitypub_path_id(&id) else {
        return not_found();
    };
    federation_actor_response(&state, id, &headers).await
}

async fn federation_actor_response(
    state: &WebState,
    account_id: i64,
    headers: &HeaderMap,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let account = match federation_account(state, account_id, false).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    if account.suspended_at.is_some() {
        let Ok(temporary) = state
            .repository
            .account_has_deletion_request(account.id)
            .await
        else {
            return internal_error();
        };
        if !temporary {
            return error_response(StatusCode::GONE, "Unavailable account");
        }
    }
    activity_response(
        StatusCode::OK,
        ACTIVITY_JSON,
        activitypub::actor(&state.origin, &state.local_domain, &account),
    )
}

async fn federation_note_username(
    State(state): State<WebState>,
    Path((username, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    if !accepts_activitypub(&headers) {
        return html_redirect(&state.origin, &format!("/@{username}/{id}"));
    }
    let account_id = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_note_response(&state, account_id, &id, &headers).await
}

async fn federation_note_id(
    State(state): State<WebState>,
    Path((account_id, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_note_response(&state, account_id, &id, &headers).await
}

#[allow(clippy::manual_let_else)]
async fn federation_note_response(
    state: &WebState,
    account_id: i64,
    id: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let Some(status_id) = route_path_id(id) else {
        return not_found();
    };
    let account = match federation_account(state, account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let Some(status) = (match state.repository.status(status_id).await {
        Ok(status) => status,
        Err(_) => return internal_error(),
    }) else {
        return not_found();
    };
    if status.account_id != account_id
        || !matches!(
            status.visibility,
            crate::mastodon::StatusVisibility::Public | crate::mastodon::StatusVisibility::Unlisted
        )
    {
        return not_found();
    }
    let media = match state.repository.media_attachments(status_id).await {
        Ok(media) => media,
        Err(_) => return internal_error(),
    };
    let mention_rows = match state.repository.mentions(status_id).await {
        Ok(rows) => rows,
        Err(_) => return internal_error(),
    };
    let mut mentions = Vec::new();
    for mention in mention_rows {
        if !mention.silent
            && let Ok(Some(target)) = state.repository.account(mention.account_id).await
        {
            mentions.push((mention, target));
        }
    }
    let hashtags = match state.repository.rest_status_tag_rows(&[status_id]).await {
        Ok(rows) => rows
            .into_iter()
            .map(|row| (row.name.clone(), row.display_name.unwrap_or(row.name)))
            .collect::<Vec<_>>(),
        Err(_) => return internal_error(),
    };
    let quoted_link = match activitypub_quote_url(state, status_id).await {
        Ok(quoted_link) => quoted_link,
        Err(()) => return internal_error(),
    };
    let quoted_identifier = match activitypub_quote_uri(state, status_id).await {
        Ok(quoted_identifier) => quoted_identifier,
        Err(()) => return internal_error(),
    };
    let replies = match activitypub_replies(state, &account, &status).await {
        Ok(replies) => replies,
        Err(()) => return internal_error(),
    };
    let in_reply_to_url = match activitypub_reply_url(state, &status).await {
        Ok(in_reply_to_url) => in_reply_to_url,
        Err(()) => return internal_error(),
    };
    activity_response(
        StatusCode::OK,
        ACTIVITY_JSON,
        activitypub::note(
            &state.origin,
            &state.local_domain,
            &status,
            &account,
            &state.media_root_url,
            &media,
            &mentions,
            &hashtags,
            quoted_link.as_deref(),
            in_reply_to_url.as_deref(),
            quoted_identifier.as_deref(),
            replies,
        ),
    )
}

async fn activitypub_quote_url(state: &WebState, status_id: i64) -> Result<Option<String>, ()> {
    let target = state
        .repository
        .activitypub_quote_target(status_id)
        .await
        .map_err(|_| ())?;
    Ok(target.map(|target| {
        if target.local {
            state
                .origin
                .join(&format!("@{}/{}", target.username, target.id))
                .expect("origin is absolute")
                .to_string()
        } else {
            target.url.unwrap_or_default()
        }
    }))
}

async fn activitypub_quote_uri(state: &WebState, status_id: i64) -> Result<Option<String>, ()> {
    let target = state
        .repository
        .activitypub_quote_target(status_id)
        .await
        .map_err(|_| ())?;
    Ok(target.map(|target| {
        if let Some(uri) = target.uri {
            uri
        } else if target.local {
            activitypub::local_status_uri(
                &state.origin,
                target.account_id,
                &target.username,
                target.id_scheme,
                target.id,
            )
        } else {
            target.url.unwrap_or_default()
        }
    }))
}

async fn activitypub_reply_url(
    state: &WebState,
    status: &crate::mastodon::Status,
) -> Result<Option<String>, ()> {
    let (Some(parent_id), Some(parent_account_id)) =
        (status.in_reply_to_id, status.in_reply_to_account_id)
    else {
        return Ok(None);
    };
    let parent = state.repository.status(parent_id).await.map_err(|_| ())?;
    let account = state
        .repository
        .account(parent_account_id)
        .await
        .map_err(|_| ())?;
    Ok(parent
        .zip(account)
        .map(|(parent, account)| activitypub::status_uri(&state.origin, &account, &parent)))
}

async fn activitypub_replies(
    state: &WebState,
    account: &Account,
    status: &crate::mastodon::Status,
) -> Result<Option<serde_json::Value>, ()> {
    if account.domain.is_some() {
        return Ok(None);
    }
    let base = activitypub::replies_url(&state.origin, account, status);
    let replies = state
        .repository
        .activitypub_reply_statuses(account.id, status.id, 5)
        .await
        .map_err(|_| ())?;
    let mut items = Vec::new();
    for reply in &replies {
        if reply.local == Some(true) {
            let Some(reply_account) = state
                .repository
                .account(reply.account_id)
                .await
                .map_err(|_| ())?
            else {
                items.push(serde_json::Value::Null);
                continue;
            };
            items.push(serde_json::Value::String(activitypub::status_uri(
                &state.origin,
                &reply_account,
                reply,
            )));
        } else {
            items.push(
                reply
                    .uri
                    .clone()
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
        }
    }
    let next = replies.last().map_or_else(
        || format!("{base}?page=true&only_other_accounts=true"),
        |reply| format!("{base}?min_id={}&page=true", reply.id),
    );
    Ok(Some(serde_json::json!({
        "id": base,
        "type": "Collection",
        "first": {
            "type": "CollectionPage",
            "partOf": base,
            "items": items,
            "next": next
        }
    })))
}

async fn federation_outbox_username(
    State(state): State<WebState>,
    Path(username): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response<Body> {
    if !accepts_activitypub(&headers) {
        return html_redirect(&state.origin, &format!("/@{username}"));
    }
    let account_id = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_outbox_response(&state, account_id, parameters, &headers).await
}

async fn federation_outbox_id(
    State(state): State<WebState>,
    Path(account_id): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_outbox_response(&state, account_id, parameters, &headers).await
}

async fn federation_outbox_instance(
    State(state): State<WebState>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response<Body> {
    federation_outbox_response(&state, -99, parameters, &headers).await
}

#[allow(
    clippy::manual_let_else,
    clippy::too_many_lines,
    clippy::uninlined_format_args
)]
async fn federation_outbox_response(
    state: &WebState,
    account_id: i64,
    parameters: HashMap<String, String>,
    headers: &HeaderMap,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let account = match federation_account(state, account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let base = Url::parse(&activitypub::collection_url(
        &state.origin,
        &account,
        "outbox",
    ))
    .expect("origin is absolute");
    if !parameters
        .get("page")
        .is_some_and(|value| activitypub_truthy(value))
    {
        let total = match state.repository.activitypub_outbox_count(account_id).await {
            Ok(total) => total,
            Err(_) => return internal_error(),
        };
        return activity_response(
            StatusCode::OK,
            ACTIVITY_JSON,
            activitypub::ordered_collection(
                base.to_string(),
                total,
                format!("{}?page=true", base),
                Some(format!("{}?min_id=0&page=true", base)),
            ),
        );
    }
    let max_id = parameters
        .get("max_id")
        .and_then(|value| route_path_id(value));
    let min_id = parameters
        .get("min_id")
        .and_then(|value| route_path_id(value));
    let since_id = parameters
        .get("since_id")
        .and_then(|value| route_path_id(value));
    let statuses = match state
        .repository
        .activitypub_outbox_statuses(account_id, 20, max_id, min_id, since_id)
        .await
    {
        Ok(statuses) => statuses,
        Err(_) => return internal_error(),
    };
    let first_status_id = statuses.first().map(|status| status.id);
    let last_status_id = statuses.last().map(|status| status.id);
    let full_page = statuses.len() == 20;
    let mut items = Vec::new();
    for status in statuses {
        let note = match state.repository.account(status.account_id).await {
            Ok(Some(status_account)) => {
                let media = match state.repository.media_attachments(status.id).await {
                    Ok(media) => media,
                    Err(_) => return internal_error(),
                };
                let mention_rows = match state.repository.mentions(status.id).await {
                    Ok(mention_rows) => mention_rows,
                    Err(_) => return internal_error(),
                };
                let mut mentions = Vec::new();
                for mention in mention_rows {
                    if !mention.silent {
                        match state.repository.account(mention.account_id).await {
                            Ok(Some(target)) => mentions.push((mention, target)),
                            Ok(None) | Err(_) => return internal_error(),
                        }
                    }
                }
                let hashtags = match state.repository.rest_status_tag_rows(&[status.id]).await {
                    Ok(rows) => rows
                        .into_iter()
                        .map(|row| (row.name.clone(), row.display_name.unwrap_or(row.name)))
                        .collect::<Vec<_>>(),
                    Err(_) => return internal_error(),
                };
                let quoted_link = match activitypub_quote_url(state, status.id).await {
                    Ok(quoted_link) => quoted_link,
                    Err(()) => return internal_error(),
                };
                let quoted_identifier = match activitypub_quote_uri(state, status.id).await {
                    Ok(quoted_identifier) => quoted_identifier,
                    Err(()) => return internal_error(),
                };
                let replies = match activitypub_replies(state, &status_account, &status).await {
                    Ok(replies) => replies,
                    Err(()) => return internal_error(),
                };
                let in_reply_to_url = match activitypub_reply_url(state, &status).await {
                    Ok(in_reply_to_url) => in_reply_to_url,
                    Err(()) => return internal_error(),
                };
                activitypub::note(
                    &state.origin,
                    &state.local_domain,
                    &status,
                    &status_account,
                    &state.media_root_url,
                    &media,
                    &mentions,
                    &hashtags,
                    quoted_link.as_deref(),
                    in_reply_to_url.as_deref(),
                    quoted_identifier.as_deref(),
                    replies,
                )
            }
            Ok(None) | Err(_) => return internal_error(),
        };
        let actor = activitypub::actor_url(&state.origin, &account);
        let item_id = if status.reblog_of_id.is_some() {
            note["id"].clone()
        } else {
            serde_json::Value::String(format!(
                "{}{}",
                note["id"].as_str().unwrap_or_default(),
                "/activity"
            ))
        };
        let item = if let Some(source_id) = status.reblog_of_id {
            let object = match state.repository.status(source_id).await {
                Ok(Some(source)) => match state.repository.account(source.account_id).await {
                    Ok(Some(source_account)) => Some(activitypub::status_uri(
                        &state.origin,
                        &source_account,
                        &source,
                    )),
                    Ok(None) | Err(_) => return internal_error(),
                },
                Ok(None) | Err(_) => return internal_error(),
            };
            let Some(object) = object else {
                return internal_error();
            };
            serde_json::json!({
                "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                "id": item_id,
                "type": "Announce",
                "actor": actor,
                "published": note["published"],
                "to": note["to"],
                "cc": note["cc"],
                "object": object
            })
        } else {
            serde_json::json!({
                "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                "id": item_id,
                "type": "Create",
                "actor": actor,
                "published": note["published"],
                "to": note["to"],
                "cc": note["cc"],
                "object": note
            })
        };
        items.push(item);
    }
    let id = outbox_page_url(&base, max_id, min_id, since_id);
    let next = full_page
        .then_some(last_status_id)
        .flatten()
        .map(|id| outbox_page_url(&base, Some(id), None, None));
    let prev = first_status_id.map(|id| outbox_page_url(&base, None, Some(id), None));
    activity_response(
        StatusCode::OK,
        ACTIVITY_JSON,
        activitypub::ordered_page(id, base.to_string(), None, items, next, prev),
    )
}

fn outbox_page_url(
    base: &Url,
    max_id: Option<i64>,
    min_id: Option<i64>,
    since_id: Option<i64>,
) -> String {
    let mut query = Vec::new();
    if let Some(id) = max_id {
        query.push(format!("max_id={id}"));
    }
    if let Some(id) = min_id {
        query.push(format!("min_id={id}"));
    }
    if let Some(id) = since_id {
        query.push(format!("since_id={id}"));
    }
    query.push("page=true".to_owned());
    format!("{}?{}", base, query.join("&"))
}

async fn federation_followers_username(
    State(state): State<WebState>,
    Path(username): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response<Body> {
    federation_collection_username(&state, username, true, parameters, &headers).await
}

async fn federation_following_username(
    State(state): State<WebState>,
    Path(username): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response<Body> {
    federation_collection_username(&state, username, false, parameters, &headers).await
}

async fn federation_collection_username(
    state: &WebState,
    username: String,
    followers: bool,
    parameters: HashMap<String, String>,
    headers: &HeaderMap,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        let collection = if followers { "followers" } else { "following" };
        return html_redirect(&state.origin, &format!("/@{username}/{collection}"));
    }
    let account_id = match federation_local_account_id(state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_collection_response(state, account_id, followers, parameters, headers).await
}

async fn federation_followers_id(
    State(state): State<WebState>,
    Path(account_id): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_collection_response(&state, account_id, true, parameters, &headers).await
}

async fn federation_following_id(
    State(state): State<WebState>,
    Path(account_id): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_collection_response(&state, account_id, false, parameters, &headers).await
}

#[allow(clippy::manual_let_else, clippy::uninlined_format_args)]
async fn federation_collection_response(
    state: &WebState,
    account_id: i64,
    followers: bool,
    parameters: HashMap<String, String>,
    headers: &HeaderMap,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let account = match federation_account(state, account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let route = if followers { "followers" } else { "following" };
    let base = Url::parse(&activitypub::collection_url(&state.origin, &account, route))
        .expect("origin is absolute");
    let total = match state
        .repository
        .activitypub_follow_count(account_id, followers)
        .await
    {
        Ok(total) => total,
        Err(_) => return internal_error(),
    };
    let paged = parameters.contains_key("page");
    if account.hide_collections.unwrap_or(false) && paged {
        return error_response(StatusCode::FORBIDDEN, "Collection is private");
    }
    if !paged {
        let Some(first) =
            (!account.hide_collections.unwrap_or(false)).then(|| format!("{}?page=1", base))
        else {
            return activity_response(
                StatusCode::OK,
                ACTIVITY_JSON,
                serde_json::json!({
                    "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                    "id": base.to_string(),
                    "type": "OrderedCollection",
                    "totalItems": total
                }),
            );
        };
        return activity_response(
            StatusCode::OK,
            ACTIVITY_JSON,
            activitypub::ordered_collection(base.to_string(), total, first, None),
        );
    }
    let offset = parameters
        .get("offset")
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| {
            parameters
                .get("page")
                .and_then(|value| value.parse::<i64>().ok())
                .map(|page| page.saturating_sub(1).saturating_mul(12))
        })
        .unwrap_or(0)
        .max(0);
    let ids = match state
        .repository
        .activitypub_follow_account_ids(account_id, followers, 12, offset)
        .await
    {
        Ok(ids) => ids,
        Err(_) => return internal_error(),
    };
    let mut items = Vec::new();
    for id in ids {
        if let Ok(Some(target)) = state.repository.account(id).await {
            items.push(serde_json::Value::String(activitypub::actor_url(
                &state.origin,
                &target,
            )));
        }
    }
    let page = offset / 12 + 1;
    let id = format!("{}?page={page}", base);
    let next = (items.len() == 12).then(|| format!("{}?page={}", base, page + 1));
    let prev = (page > 1).then(|| format!("{}?page={}", base, page - 1));
    activity_response(
        StatusCode::OK,
        ACTIVITY_JSON,
        activitypub::ordered_page(id, base.to_string(), Some(total), items, next, prev),
    )
}

fn accepts_activitypub(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get("accept").and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let mut best_activity = None;
    let mut best_html = None;
    for (index, entry) in value.split(',').enumerate() {
        let mut parameters = entry.split(';');
        let media_type = parameters.next().unwrap_or_default().trim();
        let is_activity = media_type.eq_ignore_ascii_case("application/activity+json")
            || media_type.eq_ignore_ascii_case("application/ld+json")
            || media_type.eq_ignore_ascii_case("application/json");
        let is_html = media_type.eq_ignore_ascii_case("text/html");
        if !is_activity && !is_html {
            continue;
        }
        let quality = parameters
            .find_map(|parameter| {
                let (name, value) = parameter.trim().split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("q")
                    .then_some(value.trim())
            })
            .map_or(1.0, |quality| {
                quality
                    .parse::<f32>()
                    .ok()
                    .filter(|quality| quality.is_finite())
                    .unwrap_or(0.0)
            });
        if quality <= 0.0 {
            continue;
        }
        let best = if is_activity {
            &mut best_activity
        } else {
            &mut best_html
        };
        if best.is_none_or(|(current, current_index)| {
            matches!(quality.total_cmp(&current), std::cmp::Ordering::Greater)
                || (quality.total_cmp(&current) == std::cmp::Ordering::Equal
                    && index < current_index)
        }) {
            *best = Some((quality, index));
        }
    }
    best_activity.is_some_and(|(activity_quality, activity_index)| {
        best_html.is_none_or(|(html_quality, html_index)| {
            matches!(
                activity_quality.total_cmp(&html_quality),
                std::cmp::Ordering::Greater
            ) || (activity_quality.total_cmp(&html_quality) == std::cmp::Ordering::Equal
                && activity_index < html_index)
        })
    })
}

fn activitypub_truthy(value: &str) -> bool {
    !value.is_empty() && !matches!(value, "0" | "f" | "F" | "false" | "FALSE" | "off" | "OFF")
}

fn html_redirect(origin: &Url, location: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header(
            LOCATION,
            origin
                .join(location)
                .expect("origin and redirect path are valid")
                .as_str(),
        )
        .header(VARY, "Origin, Accept")
        .body(Body::empty())
        .expect("HTML redirect response headers are valid")
}

#[allow(clippy::needless_pass_by_value)]
fn activity_response(
    status: StatusCode,
    content_type: &str,
    value: serde_json::Value,
) -> Response<Body> {
    raw_response(
        status,
        content_type,
        serde_json::to_vec(&value).expect("ActivityPub value is serializable"),
    )
}

fn raw_response(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header(VARY, "Accept")
        .body(Body::from(body))
        .expect("federation response headers are valid")
}

async fn request_context(
    State(state): State<WebState>,
    mut request: Request,
    next: Next,
) -> Response<Body> {
    let has_peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(
            || SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
            |ConnectInfo(peer)| *peer,
        );
    let metadata = request_metadata(peer, request.headers(), &state.trusted_proxies);
    for name in FORWARDED_HEADERS {
        request.headers_mut().remove(*name);
    }
    let Ok(metadata) = metadata else {
        return json_response(
            StatusCode::BAD_REQUEST,
            br#"{"error":"Invalid forwarded request metadata"}"#.to_vec(),
        );
    };
    if has_peer && !matches!(request.uri().path(), "/health" | "/ready") {
        let direct_host = request
            .headers()
            .get(HOST)
            .and_then(|value| value.to_str().ok());
        let effective_host = metadata.host.as_deref().or(direct_host);
        if effective_host.is_none_or(|host| {
            !state
                .allowed_hosts
                .iter()
                .any(|allowed| host.eq_ignore_ascii_case(allowed))
        }) {
            return json_response(
                StatusCode::MISDIRECTED_REQUEST,
                br#"{"error":"Unrecognized request host"}"#.to_vec(),
            );
        }
    }
    request.extensions_mut().insert(metadata);
    next.run(request).await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    const fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RangeSelection {
    Full,
    Ranges(Vec<ByteRange>),
    Unsatisfiable,
}

async fn paperclip_media(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if state
        .media_route_authority
        .as_deref()
        .is_some_and(|expected| {
            metadata
                .host
                .as_deref()
                .or_else(|| headers.get(HOST).and_then(|value| value.to_str().ok()))
                .is_none_or(|actual| !actual.eq_ignore_ascii_case(expected))
        })
    {
        return finalize_api_response(uri.path(), &headers, api_not_found());
    }
    if !matches!(method, Method::GET | Method::HEAD) {
        return not_found();
    }
    let Some(path) = uri
        .path()
        .strip_prefix(&state.media_route_path)
        .and_then(|path| path.strip_prefix('/'))
        .and_then(parse_paperclip_path)
    else {
        return not_found();
    };
    match state
        .repository
        .paperclip_metadata(path.attachment(), path.id())
        .await
    {
        Ok(Some(metadata)) if path.authorizes(&metadata) => {}
        Ok(_) => return not_found(),
        Err(_) => return internal_error(),
    }
    let root = state.media_root.clone();
    let relative_path = path.relative_path().to_owned();
    let opened = tokio::task::spawn_blocking(move || {
        let file = root.open_file(&relative_path)?;
        let metadata = file.metadata()?;
        Ok::<_, std::io::Error>((file, metadata))
    })
    .await;
    let (file, file_metadata) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(_)) => return not_found(),
        Err(_) => return internal_error(),
    };
    paperclip_file_response(
        &method,
        &headers,
        path.relative_path(),
        file,
        &file_metadata,
    )
}

fn paperclip_file_response(
    method: &Method,
    headers: &HeaderMap,
    path: &std::path::Path,
    file: File,
    file_metadata: &std::fs::Metadata,
) -> Response<Body> {
    let size = file_metadata.len();
    let modified = file_metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let last_modified = httpdate::fmt_http_date(modified);
    let content_type = paperclip_content_type(path.to_string_lossy().as_ref());
    let conditional = not_modified(headers, &last_modified, modified);
    if conditional == NotModified::Exact {
        let response = paperclip_response(
            StatusCode::NOT_MODIFIED,
            content_type,
            &last_modified,
            None,
            None,
            Body::empty(),
        );
        return response;
    }

    let range_header = headers.get(RANGE).and_then(|value| value.to_str().ok());
    match paperclip_ranges(range_header, size) {
        RangeSelection::Full => {
            if conditional == NotModified::Parsed && !headers.contains_key(IF_NONE_MATCH) {
                let mut response = paperclip_response(
                    StatusCode::NOT_MODIFIED,
                    content_type,
                    &last_modified,
                    None,
                    None,
                    Body::empty(),
                );
                response.headers_mut().insert(
                    LAST_MODIFIED,
                    HeaderValue::from_str(&last_modified).expect("an HTTP date is a valid header"),
                );
                return response;
            }
            let body = if method == Method::HEAD {
                Body::empty()
            } else {
                file_body(file, 0, size)
            };
            paperclip_response(
                StatusCode::OK,
                content_type,
                &last_modified,
                Some(size),
                None,
                body,
            )
        }
        RangeSelection::Ranges(ranges) if ranges.len() == 1 => {
            let range = ranges[0];
            let body = if method == Method::HEAD {
                Body::empty()
            } else {
                file_body(file, range.start, range.len())
            };
            paperclip_response(
                StatusCode::PARTIAL_CONTENT,
                content_type,
                &last_modified,
                Some(range.len()),
                Some(format!("bytes {}-{}/{}", range.start, range.end, size)),
                body,
            )
        }
        RangeSelection::Ranges(ranges) => {
            let content_length = multipart_content_length(content_type, size, &ranges);
            let body = if method == Method::HEAD {
                Body::empty()
            } else {
                multipart_body(file, content_type, size, &ranges)
            };
            paperclip_response(
                StatusCode::PARTIAL_CONTENT,
                content_type,
                &last_modified,
                Some(content_length),
                None,
                body,
            )
        }
        // Rack marks its 416 as `X-Cascade: pass`, so Rails replaces it with this 404.
        RangeSelection::Unsatisfiable => framework_not_found(),
    }
}

fn paperclip_response(
    status: StatusCode,
    content_type: &'static str,
    last_modified: &str,
    content_length: Option<u64>,
    content_range: Option<String>,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder()
        .status(status)
        .header(CACHE_CONTROL, PAPERCLIP_CACHE)
        .header("content-security-policy", PAPERCLIP_CSP)
        .header("x-content-type-options", "nosniff")
        .body(body)
        .expect("static Paperclip headers are valid");
    if status != StatusCode::NOT_MODIFIED {
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        response.headers_mut().insert(
            LAST_MODIFIED,
            HeaderValue::from_str(last_modified).expect("an HTTP date is a valid header"),
        );
        if let Some(content_length) = content_length {
            response.headers_mut().insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(&content_length.to_string())
                    .expect("a decimal length is a valid header"),
            );
        }
        if let Some(content_range) = content_range {
            response.headers_mut().insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&content_range).expect("a byte range is a valid header"),
            );
        }
    }
    response
}

fn file_body(file: File, start: u64, length: u64) -> Body {
    let stream = futures_util::stream::try_unfold(
        (file, Some(start), length),
        |(mut file, start, remaining)| async move {
            if remaining == 0 {
                return Ok(None);
            }
            tokio::task::spawn_blocking(move || {
                if let Some(start) = start {
                    file.seek(SeekFrom::Start(start))?;
                }
                let capacity = usize::try_from(remaining.min(64 * 1024)).unwrap_or(64 * 1024);
                let mut buffer = vec![0_u8; capacity];
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Paperclip file ended before its reported size",
                    ));
                }
                buffer.truncate(count);
                Ok(Some((
                    Bytes::from(buffer),
                    (file, None, remaining - count as u64),
                )))
            })
            .await
            .map_err(std::io::Error::other)?
        },
    );
    Body::from_stream(stream)
}

fn multipart_body(file: File, content_type: &str, size: u64, ranges: &[ByteRange]) -> Body {
    let mut parts = VecDeque::new();
    for range in ranges {
        parts.push_back(MultipartPart::Bytes(Bytes::from(multipart_prefix(
            content_type,
            size,
            *range,
        ))));
        parts.push_back(MultipartPart::File {
            start: range.start,
            remaining: range.len(),
        });
    }
    parts.push_back(MultipartPart::Bytes(Bytes::from(format!(
        "\r\n--{MULTIPART_BOUNDARY}--\r\n"
    ))));
    let stream =
        futures_util::stream::try_unfold((file, parts), |(mut file, mut parts)| async move {
            let Some(part) = parts.pop_front() else {
                return Ok::<_, std::io::Error>(None);
            };
            match part {
                MultipartPart::Bytes(bytes) => Ok(Some((bytes, (file, parts)))),
                MultipartPart::File { start, remaining } => {
                    tokio::task::spawn_blocking(move || {
                        file.seek(SeekFrom::Start(start))?;
                        let capacity =
                            usize::try_from(remaining.min(64 * 1024)).unwrap_or(64 * 1024);
                        let mut buffer = vec![0_u8; capacity];
                        file.read_exact(&mut buffer)?;
                        if remaining > capacity as u64 {
                            parts.push_front(MultipartPart::File {
                                start: start + capacity as u64,
                                remaining: remaining - capacity as u64,
                            });
                        }
                        Ok(Some((Bytes::from(buffer), (file, parts))))
                    })
                    .await
                    .map_err(std::io::Error::other)?
                }
            }
        });
    Body::from_stream(stream)
}

enum MultipartPart {
    Bytes(Bytes),
    File { start: u64, remaining: u64 },
}

fn multipart_content_length(content_type: &str, size: u64, ranges: &[ByteRange]) -> u64 {
    ranges
        .iter()
        .map(|range| multipart_prefix(content_type, size, *range).len() as u64 + range.len())
        .sum::<u64>()
        + format!("\r\n--{MULTIPART_BOUNDARY}--\r\n").len() as u64
}

fn multipart_prefix(content_type: &str, size: u64, range: ByteRange) -> String {
    format!(
        "\r\n--{MULTIPART_BOUNDARY}\r\ncontent-type: {content_type}\r\ncontent-range: bytes {}-{}/{size}\r\n\r\n",
        range.start, range.end
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NotModified {
    No,
    Exact,
    Parsed,
}

fn not_modified(headers: &HeaderMap, formatted: &str, modified: SystemTime) -> NotModified {
    let Some(value) = headers
        .get(IF_MODIFIED_SINCE)
        .and_then(|value| value.to_str().ok())
    else {
        return NotModified::No;
    };
    if value == formatted {
        return NotModified::Exact;
    }
    if httpdate::parse_http_date(value).is_ok_and(|requested| {
        requested
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .zip(modified.duration_since(SystemTime::UNIX_EPOCH).ok())
            .is_some_and(|(requested, modified)| requested.as_secs() >= modified.as_secs())
    }) {
        NotModified::Parsed
    } else {
        NotModified::No
    }
}

fn paperclip_ranges(header: Option<&str>, size: u64) -> RangeSelection {
    let Some(value) = header.filter(|_| size > 0) else {
        return RangeSelection::Full;
    };
    let Some((_, value)) = value.split_once("bytes=") else {
        return RangeSelection::Full;
    };
    let value = value.split(';').next().unwrap_or_default();
    if value.bytes().filter(|byte| *byte == b',').count() >= 100 {
        return RangeSelection::Full;
    }
    let mut specifications = value.split(',').collect::<Vec<_>>();
    while specifications.last().is_some_and(|value| value.is_empty()) {
        specifications.pop();
    }
    let mut ranges = Vec::new();
    for value in specifications {
        let Some((start, end)) = value.trim().split_once('-') else {
            return RangeSelection::Full;
        };
        let range = if start.is_empty() {
            let suffix = RubyUnsigned::parse(end);
            if suffix.is_zero() {
                continue;
            }
            ByteRange {
                start: suffix
                    .as_u64()
                    .map_or(0, |suffix| size.saturating_sub(suffix)),
                end: size - 1,
            }
        } else {
            let start = RubyUnsigned::parse(start);
            let explicit_end = !end.is_empty();
            let end = if explicit_end {
                RubyUnsigned::parse(end)
            } else {
                RubyUnsigned::from_u64(size - 1)
            };
            if explicit_end && end < start {
                return RangeSelection::Full;
            }
            let Some(start) = start.as_u64().filter(|start| *start < size) else {
                continue;
            };
            ByteRange {
                start,
                end: end.as_u64().map_or(size - 1, |end| end.min(size - 1)),
            }
        };
        ranges.push(range);
    }
    if ranges.is_empty() || ranges.iter().map(|range| range.len()).sum::<u64>() > size {
        RangeSelection::Unsatisfiable
    } else {
        RangeSelection::Ranges(ranges)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RubyUnsigned(String);

impl Ord for RubyUnsigned {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for RubyUnsigned {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl RubyUnsigned {
    fn parse(value: &str) -> Self {
        let value = value
            .trim_start()
            .strip_prefix('+')
            .unwrap_or(value.trim_start());
        let digits = value
            .bytes()
            .take_while(u8::is_ascii_digit)
            .collect::<Vec<_>>();
        let digits = std::str::from_utf8(&digits).unwrap_or_default();
        let digits = digits.trim_start_matches('0');
        Self(if digits.is_empty() { "0" } else { digits }.to_owned())
    }

    fn from_u64(value: u64) -> Self {
        Self(value.to_string())
    }

    fn is_zero(&self) -> bool {
        self.0 == "0"
    }

    fn as_u64(&self) -> Option<u64> {
        self.0.parse().ok()
    }
}

fn paperclip_content_type(file_name: &str) -> &'static str {
    match file_name
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "avif" => "image/avif",
        "svg" | "svgz" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "apng" => "image/apng",
        "btif" => "image/prs.btif",
        "cgm" => "image/cgm",
        "cmx" => "image/x-cmx",
        "djv" | "djvu" => "image/vnd.djvu",
        "dwg" => "image/vnd.dwg",
        "dxf" => "image/vnd.dxf",
        "fbs" => "image/vnd.fastbidsheet",
        "flif" => "image/flif",
        "fpx" => "image/vnd.fpx",
        "fst" => "image/vnd.fst",
        "g3" => "image/g3fax",
        "heics" => "image/heic-sequence",
        "heifs" => "image/heif-sequence",
        "ief" => "image/ief",
        "jp2" => "image/jp2",
        "jpm" => "video/jpm",
        "mdi" => "image/vnd.ms-modi",
        "mj2" => "video/mj2",
        "mmr" => "image/vnd.fujixerox.edmics-mmr",
        "npx" => "image/vnd.net-fpx",
        "pbm" => "image/x-portable-bitmap",
        "pcx" => "image/x-pcx",
        "pgm" => "image/x-portable-graymap",
        "pic" => "image/x-pict",
        "pict" => "image/pict",
        "pnm" => "image/x-portable-anymap",
        "pntg" => "image/x-macpaint",
        "ppm" => "image/x-portable-pixmap",
        "psd" => "image/vnd.adobe.photoshop",
        "qtif" => "image/x-quicktime",
        "ras" => "image/x-cmu-raster",
        "rgb" => "image/x-rgb",
        "rlc" => "image/vnd.fujixerox.edmics-rlc",
        "wbmp" => "image/vnd.wap.wbmp",
        "xbm" => "image/x-xbitmap",
        "xif" => "image/vnd.xiff",
        "xpm" => "image/x-xpixmap",
        "xwd" => "image/x-xwindowdump",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "ogg" => "application/ogg",
        "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/x-wav",
        "m4a" => "audio/mp4a-latm",
        "3gp" => "video/3gpp",
        "wma" => "audio/x-ms-wma",
        "ico" => "image/vnd.microsoft.icon",
        _ => "text/plain",
    }
}

fn api_route(path: &str) -> Option<&'static ApiRouteContract> {
    let path = path.strip_suffix('/').unwrap_or(path);
    if reserved_static_route(path) {
        return API_ROUTE_INVENTORY.iter().find(|route| route.path == path);
    }
    API_ROUTE_INVENTORY
        .iter()
        .find(|route| route_path_matches(route.path, path))
}

fn preflight_api_route(path: &str) -> Option<&'static ApiRouteContract> {
    let path = path.strip_suffix('/').unwrap_or(path);
    if reserved_static_route(path) {
        return API_ROUTE_INVENTORY.iter().find(|route| route.path == path);
    }
    API_ROUTE_INVENTORY
        .iter()
        .find(|route| preflight_route_path_matches(route.path, path))
}

fn reserved_static_route(path: &str) -> bool {
    const RESERVED_SEGMENTS: &[&str] = &[
        "lookup",
        "relationships",
        "familiar_followers",
        "search",
        "update_credentials",
        "verify_credentials",
    ];
    let segments = path.split('/').collect::<Vec<_>>();
    segments.len() == 5 && RESERVED_SEGMENTS.contains(&segments[4])
}

fn route_path_matches(pattern: &str, path: &str) -> bool {
    let mut pattern = pattern.split('/');
    let mut path = path.split('/');
    loop {
        match (pattern.next(), path.next()) {
            (None, None) => return true,
            (Some(expected), Some(actual))
                if expected == actual
                    || (expected.starts_with('{')
                        && expected.ends_with('}')
                        && !actual.is_empty()) => {}
            _ => return false,
        }
    }
}

fn preflight_route_path_matches(pattern: &str, path: &str) -> bool {
    let mut pattern = pattern.split('/');
    let mut path = path.split('/');
    loop {
        match (pattern.next(), path.next()) {
            (None, None) => return true,
            (Some(expected), Some(actual)) if expected == actual => {}
            (Some("{id}"), Some(actual)) if !actual.is_empty() => {}
            (Some(expected), Some(actual))
                if expected.starts_with('{') && expected.ends_with('}') && !actual.is_empty() => {}
            _ => return false,
        }
    }
}

async fn api_protocol(request: Request, next: Next) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    if request.method() == Method::OPTIONS
        && let Some(response) = cors_preflight_response(&path, &headers)
    {
        return response;
    }
    let mut request = match bounded_request(request, REST_BODY_LIMIT_BYTES).await {
        Ok(request) => request,
        Err(response) => return finalize_api_response(&path, &headers, response),
    };
    if !valid_percent_encoded(&path) {
        return malformed_request_response(&headers);
    }
    if request
        .uri()
        .query()
        .is_some_and(|query| !valid_query(query))
    {
        return malformed_request_response(&headers);
    }
    if let Err(error) = merge_request_parameters(&mut request) {
        return match error {
            RequestParameterError::BadRequest => framework_bad_request(&headers),
            RequestParameterError::InternalServer => framework_internal_error(),
        };
    }
    let response = next.run(request).await;
    finalize_api_response(&path, &headers, response)
}

fn cors_preflight_response(path: &str, headers: &HeaderMap) -> Option<Response<Body>> {
    let requested_method = headers.get(ACCESS_CONTROL_REQUEST_METHOD)?;
    if headers.get(ORIGIN).is_none()
        || preflight_api_route(path).is_none()
        || !cors_method(requested_method.as_bytes())
    {
        return None;
    }
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(ACCESS_CONTROL_ALLOW_METHODS, CORS_METHODS)
        .header(ACCESS_CONTROL_MAX_AGE, CORS_MAX_AGE)
        .header(ACCESS_CONTROL_EXPOSE_HEADERS, CORS_EXPOSE_HEADERS)
        .body(Body::empty())
        .expect("static CORS response headers are valid");
    if let Some(requested_headers) = headers.get(ACCESS_CONTROL_REQUEST_HEADERS) {
        response
            .headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_HEADERS, requested_headers.clone());
    }
    Some(response)
}

async fn bounded_request(request: Request, limit: usize) -> Result<Request, Response<Body>> {
    let (mut parts, body) = request.into_parts();
    let mut stream = body.into_data_stream();
    let mut size = 0_usize;
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.try_next().await.map_err(|_| internal_error())? {
        size = size.saturating_add(chunk.len());
        if size > limit {
            return Err(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Payload Too Large",
            ));
        }
        buffer.extend_from_slice(&chunk);
    }
    let body = Bytes::from(buffer);
    parts.extensions.insert(BufferedRequestBody(body.clone()));
    Ok(Request::from_parts(parts, Body::from(body)))
}

#[derive(Clone)]
struct BufferedRequestBody(Bytes);

#[derive(Clone, Copy)]
enum RequestParameterError {
    BadRequest,
    InternalServer,
}

fn merge_request_parameters(request: &mut Request) -> Result<(), RequestParameterError> {
    let query_parameters = RackParameters::parse(request.uri().query().unwrap_or_default())
        .map_err(RequestParameterError::from)?;
    let body = request.extensions().get::<BufferedRequestBody>();
    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    let body_parameters = if body.is_none_or(|body| body.0.is_empty()) {
        RackParameters::default()
    } else {
        let Some(body) = body else {
            unreachable!("nonempty request body is present")
        };
        match content_type {
            Some(value) if value.eq_ignore_ascii_case("application/x-www-form-urlencoded") => {
                let body =
                    std::str::from_utf8(&body.0).map_err(|_| RequestParameterError::BadRequest)?;
                if !valid_query(body) {
                    return Err(RequestParameterError::BadRequest);
                }
                RackParameters::parse(body).map_err(RequestParameterError::from)?
            }
            Some(value) if json_content_type(value) => {
                let value = serde_json::from_slice(&body.0)
                    .map_err(|_| RequestParameterError::BadRequest)?;
                let root = RackValue::from_json(&value);
                if root.json_depth() > 100 {
                    return Err(RequestParameterError::BadRequest);
                }
                RackParameters::from_json(&value)
            }
            _ => RackParameters::default(),
        }
    };
    let merged = body_parameters.merge(query_parameters);
    let query = merged.to_query();
    let path_and_query = if query.is_empty() {
        request.uri().path().to_owned()
    } else {
        format!("{}?{query}", request.uri().path())
    };
    *request.uri_mut() = path_and_query
        .parse()
        .map_err(|_| RequestParameterError::BadRequest)?;
    request.extensions_mut().insert(merged);
    Ok(())
}

fn json_content_type(value: &str) -> bool {
    [
        "application/json",
        "text/x-json",
        "application/jsonrequest",
        "application/jrd+json",
        "application/activity+json",
        "application/ld+json",
        "application/problem+json",
    ]
    .iter()
    .any(|mime| value.eq_ignore_ascii_case(mime))
}

#[derive(Clone, Debug)]
enum RackValue {
    Null,
    Scalar(String),
    Number(serde_json::Number),
    Boolean(bool),
    Array(Vec<RackValue>),
    Object(BTreeMap<String, RackValue>),
}

#[derive(Clone, Debug, Default)]
struct RackParameters(BTreeMap<String, RackValue>);

#[derive(Clone, Debug)]
enum RackKeySegment {
    Field(String),
    TrailingPush(String),
    Push,
}

#[derive(Clone, Copy, Debug)]
enum RackParseError {
    Conflict,
    Limit,
}

impl From<RackParseError> for RequestParameterError {
    fn from(error: RackParseError) -> Self {
        match error {
            RackParseError::Conflict => Self::BadRequest,
            RackParseError::Limit => Self::InternalServer,
        }
    }
}

impl RackParameters {
    fn parse(encoded: &str) -> Result<Self, RackParseError> {
        if encoded.len() > RACK_BYTES_LIMIT {
            return Err(RackParseError::Limit);
        }
        let mut parameters = Self::default();
        for (index, raw) in encoded.split('&').enumerate() {
            if index == RACK_PARAMETER_LIMIT {
                return Err(RackParseError::Limit);
            }
            let Some((name, value)) = url::form_urlencoded::parse(raw.as_bytes()).next() else {
                continue;
            };
            let Some((root, segments)) = rack_key(&name)? else {
                continue;
            };
            let value = if raw.contains('=') {
                RackValue::Scalar(value.into_owned())
            } else {
                RackValue::Null
            };
            parameters.insert(root, &segments, value)?;
        }
        Ok(parameters)
    }

    fn from_json(value: &serde_json::Value) -> Self {
        let serde_json::Value::Object(values) = value else {
            return Self(BTreeMap::from([(
                "_json".to_owned(),
                RackValue::from_json(value),
            )]));
        };
        Self(
            values
                .iter()
                .map(|(key, value)| (key.clone(), RackValue::from_json(value)))
                .collect(),
        )
    }

    fn insert(
        &mut self,
        root: String,
        segments: &[RackKeySegment],
        value: RackValue,
    ) -> Result<(), RackParseError> {
        if segments.is_empty() {
            self.0.insert(root, value);
            return Ok(());
        }
        let expected = match segments[0] {
            RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
            RackKeySegment::Push | RackKeySegment::TrailingPush(_) => RackValue::Array(Vec::new()),
        };
        insert_rack_value(self.0.entry(root).or_insert(expected), segments, value)
    }

    fn merge(mut self, query: Self) -> Self {
        self.0.extend(query.0);
        self
    }

    fn get(&self, name: &str) -> Option<&RackValue> {
        self.0.get(name)
    }

    fn to_query(&self) -> String {
        fn append(pairs: &mut Vec<(String, String)>, name: String, value: &RackValue) {
            match value {
                RackValue::Null => {}
                RackValue::Scalar(value) => pairs.push((name, value.clone())),
                RackValue::Number(value) => pairs.push((name, ruby_json_number(value))),
                RackValue::Boolean(value) => pairs.push((name, value.to_string())),
                RackValue::Array(values) => {
                    for value in values {
                        append(pairs, format!("{name}[]"), value);
                    }
                }
                RackValue::Object(values) => {
                    for (key, value) in values {
                        append(pairs, format!("{name}[{key}]"), value);
                    }
                }
            }
        }

        let mut pairs = Vec::new();
        for (name, value) in &self.0 {
            append(&mut pairs, name.clone(), value);
        }
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.extend_pairs(pairs);
        serializer.finish()
    }
}

impl RackValue {
    fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::String(value) => Self::Scalar(value.clone()),
            serde_json::Value::Number(value) => Self::Number(value.clone()),
            serde_json::Value::Bool(value) => Self::Boolean(*value),
            serde_json::Value::Array(values) => {
                Self::Array(values.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(values) => Self::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    fn json_depth(&self) -> usize {
        match self {
            Self::Array(values) => 1 + values.iter().map(Self::json_depth).max().unwrap_or(0),
            Self::Object(values) => 1 + values.values().map(Self::json_depth).max().unwrap_or(0),
            Self::Null | Self::Scalar(_) | Self::Number(_) | Self::Boolean(_) => 0,
        }
    }
}

fn rack_key(name: &str) -> Result<Option<(String, Vec<RackKeySegment>)>, RackParseError> {
    let root_end = name
        .get(1..)
        .and_then(|suffix| suffix.find('[').map(|index| index + 1))
        .unwrap_or(name.len());
    let root = &name[..root_end];
    if root.is_empty() {
        return Ok(None);
    }
    let mut segments = Vec::new();
    let mut suffix = &name[root_end..];
    while !suffix.is_empty() {
        let Some(rest) = suffix.strip_prefix('[') else {
            break;
        };
        let end = rest.find(']').unwrap_or(rest.len());
        let field = &rest[..end];
        let remaining = if end == rest.len() {
            ""
        } else {
            &rest[end + 1..]
        };
        segments.push(if field.is_empty() {
            if !remaining.is_empty() && matches!(segments.last(), Some(RackKeySegment::Push)) {
                RackKeySegment::Field("[]".to_owned())
            } else {
                RackKeySegment::Push
            }
        } else {
            RackKeySegment::Field(field.to_owned())
        });
        suffix = remaining;
        if segments.len() >= RACK_DEPTH_LIMIT {
            return Err(RackParseError::Limit);
        }
    }
    if !suffix.is_empty() {
        match segments.last() {
            Some(RackKeySegment::Field(_)) => {
                segments.push(RackKeySegment::Field(suffix.to_owned()));
            }
            Some(RackKeySegment::Push) => {
                segments.pop();
                segments.push(RackKeySegment::TrailingPush(suffix.to_owned()));
            }
            Some(RackKeySegment::TrailingPush(_)) | None => {}
        }
    }
    if segments.len() >= RACK_DEPTH_LIMIT {
        return Err(RackParseError::Limit);
    }
    Ok(Some((root.to_owned(), segments)))
}

fn insert_rack_value(
    target: &mut RackValue,
    segments: &[RackKeySegment],
    value: RackValue,
) -> Result<(), RackParseError> {
    let Some((segment, rest)) = segments.split_first() else {
        *target = value;
        return Ok(());
    };
    match (target, segment) {
        (RackValue::Object(values), RackKeySegment::Field(field)) => {
            if rest.is_empty() {
                values.insert(field.clone(), value);
                return Ok(());
            }
            let expected = match rest[0] {
                RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
                RackKeySegment::Push | RackKeySegment::TrailingPush(_) => {
                    RackValue::Array(Vec::new())
                }
            };
            insert_rack_value(values.entry(field.clone()).or_insert(expected), rest, value)
        }
        (RackValue::Array(values), RackKeySegment::TrailingPush(field)) => {
            let path = [RackKeySegment::Field(field.clone())];
            if let Some(RackValue::Object(last)) = values.last()
                && !matches!(rack_path_state(last, &path), RackPathState::Existing)
            {
                let last = values.last_mut().expect("last array object exists");
                return insert_rack_value(last, &path, value);
            }
            let mut nested = RackValue::Object(BTreeMap::new());
            insert_rack_value(&mut nested, &path, value)?;
            values.push(nested);
            Ok(())
        }
        (RackValue::Array(values), RackKeySegment::Push) => {
            if rest.is_empty() {
                values.push(value);
                return Ok(());
            }
            if matches!(rest.first(), Some(RackKeySegment::Field(_)))
                && let Some(RackValue::Object(last)) = values.last()
                && !matches!(rack_path_state(last, rest), RackPathState::Existing)
            {
                let last = values.last_mut().expect("last array object exists");
                return insert_rack_value(last, rest, value);
            }
            if matches!(rest, [RackKeySegment::Push])
                && matches!(values.last(), Some(RackValue::Object(_)))
            {
                return Ok(());
            }
            if matches!(rest.first(), Some(RackKeySegment::Push))
                && matches!(values.last(), Some(RackValue::Array(_)))
            {
                let last = values.last_mut().expect("last nested array exists");
                return insert_rack_value(last, rest, value);
            }
            let mut nested = match rest[0] {
                RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
                RackKeySegment::Push | RackKeySegment::TrailingPush(_) => {
                    RackValue::Array(Vec::new())
                }
            };
            insert_rack_value(&mut nested, rest, value)?;
            values.push(nested);
            Ok(())
        }
        _ => Err(RackParseError::Conflict),
    }
}

#[derive(Clone, Copy)]
enum RackPathState {
    Missing,
    Existing,
    Conflict,
}

fn rack_path_state(
    values: &BTreeMap<String, RackValue>,
    segments: &[RackKeySegment],
) -> RackPathState {
    let Some((segment, rest)) = segments.split_first() else {
        return RackPathState::Existing;
    };
    let RackKeySegment::Field(field) = segment else {
        return RackPathState::Conflict;
    };
    let Some(value) = values.get(field) else {
        return RackPathState::Missing;
    };
    let Some((next, _)) = rest.split_first() else {
        return RackPathState::Existing;
    };
    match (value, next) {
        (RackValue::Object(values), RackKeySegment::Field(_)) => rack_path_state(values, rest),
        (RackValue::Array(_), RackKeySegment::Push | RackKeySegment::TrailingPush(_)) => {
            RackPathState::Missing
        }
        _ => RackPathState::Conflict,
    }
}

fn cors_method(method: &[u8]) -> bool {
    ["POST", "PUT", "DELETE", "GET", "PATCH", "OPTIONS"]
        .iter()
        .any(|allowed| method.eq_ignore_ascii_case(allowed.as_bytes()))
}

fn valid_query(query: &str) -> bool {
    valid_percent_encoded(query)
}

fn valid_percent_encoded(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    percent_decode_str(value).decode_utf8().is_ok()
}

fn malformed_request_response(request_headers: &HeaderMap) -> Response<Body> {
    let mut response = Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header(CONTENT_TYPE, "application/json; charset=UTF-8")
        .header(VARY, "Origin")
        .body(Body::from(r#"{"status":400,"error":"Bad Request"}"#))
        .expect("static malformed-query response is valid");
    if request_headers.contains_key(ORIGIN) {
        response
            .headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(CORS_METHODS),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(CORS_EXPOSE_HEADERS),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static(CORS_MAX_AGE),
        );
    }
    response
}

fn framework_bad_request(request_headers: &HeaderMap) -> Response<Body> {
    let mut response = malformed_request_response(request_headers);
    response
        .headers_mut()
        .insert(FRAMEWORK_ERROR_HEADER, HeaderValue::from_static("1"));
    response
}

fn finalize_api_response(
    path: &str,
    request_headers: &HeaderMap,
    mut response: Response<Body>,
) -> Response<Body> {
    if response
        .headers_mut()
        .remove(FRAMEWORK_ERROR_HEADER)
        .is_some()
    {
        if request_headers.contains_key(ORIGIN) {
            response
                .headers_mut()
                .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        }
        response
            .headers_mut()
            .insert(VARY, HeaderValue::from_static("Origin"));
        return response;
    }
    let Some(route) = api_route(path) else {
        return response;
    };
    let authenticated = request_headers
        .get(AUTHORIZATION)
        .is_some_and(|value| !value.as_bytes().iter().all(u8::is_ascii_whitespace));
    let success = response.status().is_success();
    let cache_control = match (success, route.cache, authenticated) {
        (true, ApiCachePolicy::Public, _) => PUBLIC_CACHE,
        (true, ApiCachePolicy::Anonymous, false) => ANONYMOUS_CACHE,
        _ => PRIVATE_CACHE,
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    let cors = request_headers.contains_key(ORIGIN);
    let public_vary = matches!(route.cache, ApiCachePolicy::Public);
    let vary = if public_vary {
        Some("Accept, Origin")
    } else {
        Some("Authorization, Origin")
    };
    if let Some(vary) = vary {
        response
            .headers_mut()
            .insert(VARY, HeaderValue::from_static(vary));
    } else {
        response.headers_mut().remove(VARY);
    }
    if cors {
        response
            .headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        response.headers_mut().insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(CORS_EXPOSE_HEADERS),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(CORS_METHODS),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static(CORS_MAX_AGE),
        );
    }
    response
}

/// Serves the currently implemented Rustodon HTTP surface.
///
/// # Errors
///
/// Returns an I/O error when binding or serving the listener fails.
pub async fn serve<F>(address: SocketAddr, state: WebState, shutdown: F) -> std::io::Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}

async fn health() -> Response<Body> {
    json_response(StatusCode::OK, br#"{"status":"ok"}"#.to_vec())
}

async fn readiness(State(state): State<WebState>) -> Response<Body> {
    if state.repository.ready().await {
        json_response(StatusCode::OK, br#"{"status":"ready"}"#.to_vec())
    } else {
        json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            br#"{"status":"not_ready"}"#.to_vec(),
        )
    }
}

async fn instance_v1(State(state): State<WebState>) -> Response<Body> {
    instance_response(state, true).await
}

async fn instance_v2(State(state): State<WebState>) -> Response<Body> {
    instance_response(state, false).await
}

async fn instance_response(state: WebState, v1: bool) -> Response<Body> {
    let Ok(instance) = state
        .loader(None)
        .instance(state.instance_runtime.clone())
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let body = if v1 {
        serializer
            .instance_v1(&instance)
            .ok()
            .and_then(|value| serde_json::to_vec(&value).ok())
    } else {
        serializer
            .instance_v2(&instance)
            .ok()
            .and_then(|value| serde_json::to_vec(&value).ok())
    };
    body.map_or_else(internal_error, |body| json_response(StatusCode::OK, body))
}

async fn instance_rules(State(state): State<WebState>) -> Response<Body> {
    let Ok(instance) = state
        .loader(None)
        .instance(state.instance_runtime.clone())
        .await
    else {
        return internal_error();
    };
    match serde_json::to_vec(&RestSerializer::rules(&instance)) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn translation_languages() -> Response<Body> {
    json_response(StatusCode::OK, b"{}".to_vec())
}

async fn account_show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    match state.repository.rest_account_showable(account_id).await {
        Ok(true) => {}
        Ok(false) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    account_response(&state, state.loader(viewer).account(account_id).await)
}

async fn collection_show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_COLLECTIONS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, collection_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    match state
        .repository
        .rest_collection_showable(collection_id, viewer)
        .await
    {
        Ok(Some(true)) => {}
        Ok(Some(false)) => {
            return error_response(StatusCode::FORBIDDEN, "This action is not allowed");
        }
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    let collection = match state.loader(viewer).collection(collection_id).await {
        Ok(Some(collection)) => collection,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .collection_with_accounts(&collection)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn account_lookup(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let handle = match rack.get("acct") {
        None | Some(RackValue::Null) => None,
        Some(RackValue::Scalar(value)) => Some(value.clone()),
        Some(
            RackValue::Number(_)
            | RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_),
        ) => return framework_internal_error(),
    };
    let account = match handle {
        Some(handle) => state.loader(viewer).lookup_account(&handle).await,
        None => Ok(None),
    };
    account_response(&state, account)
}

async fn account_search(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let query = match rack.get("q") {
        None | Some(RackValue::Null) => None,
        Some(RackValue::Scalar(value)) => Some(value.as_str()),
        Some(
            RackValue::Number(_)
            | RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_),
        ) => return framework_internal_error(),
    };
    let offset = match rack.get("offset") {
        None | Some(RackValue::Null) => 0,
        Some(RackValue::Scalar(value)) => ruby_integer(value),
        Some(RackValue::Number(value)) => json_number_integer(value).unwrap_or(0),
        Some(RackValue::Boolean(_) | RackValue::Array(_) | RackValue::Object(_)) => {
            return framework_internal_error();
        }
    };
    if offset < 0 {
        return framework_internal_error();
    }
    let Ok(limit) = limit_parameter(&rack, 40, 80) else {
        return framework_internal_error();
    };
    let resolve = boolean_parameter(&rack, "resolve");
    let accounts = match state
        .loader(Some(owner))
        .account_search(
            query,
            resolve,
            boolean_parameter(&rack, "following"),
            limit,
            offset,
        )
        .await
    {
        Ok(accounts) => accounts,
        Err(AccountSearchError::RemoteResolutionUnsupported) => {
            return json_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"error":"Remote account resolution is unavailable"}"#.to_vec(),
            );
        }
        Err(AccountSearchError::Database(_)) => return internal_error(),
    };
    let serializer = state.serializer();
    let values = accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>();
    match values
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn markers(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer_owner(&state, &headers, READ_STATUSES).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let timelines = rack_array_values(&rack, "timeline");
    let Ok(markers) = state
        .loader(Some(owner.account_id()))
        .markers(owner.user_id(), &timelines)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = markers
        .iter()
        .map(|marker| (marker.timeline.clone(), serializer.marker(marker)))
        .collect::<BTreeMap<_, _>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn filters(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_FILTERS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(filters) = state.loader(Some(owner)).filters().await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = filters
        .iter()
        .map(|filter| serializer.filter(filter))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn lists(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_LISTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(lists) = state.loader(Some(owner)).lists(owner).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = lists
        .iter()
        .map(|list| serializer.list(list))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn featured_tags(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(tags) = state.loader(Some(owner)).featured_tags(owner).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = tags
        .iter()
        .map(|tag| serializer.featured_tag(tag))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn account_featured_tags(
    State(state): State<WebState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if let Err(response) = optional_suspension_guard(&state, &headers).await {
        return response;
    }
    let Some((_, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let account = match state.repository.account(account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if account.suspended_at.is_some() {
        return json_response(StatusCode::OK, b"[]".to_vec());
    }
    let Ok(tags) = state.loader(None).featured_tags(account_id).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = tags
        .iter()
        .map(|tag| serializer.featured_tag(tag))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn optional_suspension_guard(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<(), Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Ok(());
    }
    match state.authenticator.authenticate(headers, NO_SCOPE).await {
        Ok(_) => Ok(()),
        Err(OAuthAuthenticationError::OAuth(error)) => match error {
            OAuthError::UserDisabled => Err(OAuthError::UserDisabled
                .into_http_response()
                .map(Body::from)),
            _ => Ok(()),
        },
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

fn account_response(
    state: &WebState,
    account: sqlx::Result<Option<crate::mastodon::rest::AccountProjection>>,
) -> Response<Body> {
    let account = match account {
        Ok(Some(account)) => account,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .account(&account)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn relationships(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_FOLLOWS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(ids) = relationship_ids(&rack) else {
        return framework_internal_error();
    };
    let with_suspended = boolean_parameter(&rack, "with_suspended");
    let Ok(values) = state
        .loader(Some(owner))
        .relationships(&ids, with_suspended)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = values
        .iter()
        .map(|value| serializer.relationship(value))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn verify_credentials(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer_owner(&state, &headers, VERIFY_CREDENTIALS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let credential = match state
        .loader(Some(owner.account_id()))
        .credential_account(owner.user_id(), owner.account_id())
        .await
    {
        Ok(Some(credential)) => credential,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .credential_account(&credential)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn account_statuses(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((account_path, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let loader = state.loader(viewer);
    match loader.account(account_id).await {
        Ok(Some(account)) if account.suspended => return statuses_response(&state, &[]),
        Ok(Some(_)) => {}
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    let parameters = parameters(query.as_deref());
    let (max_id, min_id, since_id) = match cursor_triplet(&rack) {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = AccountStatusesOptions {
        max_id,
        min_id,
        since_id,
        limit: match limit_parameter(&rack, 20, 40) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
        pinned: boolean_parameter(&rack, "pinned"),
        tagged: tagged_parameter(&rack),
        only_media: boolean_parameter(&rack, "only_media"),
        exclude_replies: boolean_parameter(&rack, "exclude_replies"),
        exclude_reblogs: boolean_parameter(&rack, "exclude_reblogs"),
        exclude_direct: boolean_parameter(&rack, "exclude_direct"),
    };
    let Ok(statuses) = loader.account_statuses(account_id, &options).await else {
        return internal_error();
    };
    let first_id = statuses.first().map(|status| status.id);
    let last_id = statuses.last().map(|status| status.id);
    let mut response = statuses_response(&state, &statuses);
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| statuses.len() == limit)
        && let Some(last_id) = last_id
        && let Some(url) = pagination_url(
            &state,
            &format!("api/v1/accounts/{account_path}/statuses"),
            &parameters,
            "max_id",
            last_id,
            &[
                "limit",
                "pinned",
                "tagged",
                "only_media",
                "exclude_replies",
                "exclude_reblogs",
            ],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_id) = first_id
        && let Some(url) = pagination_url(
            &state,
            &format!("api/v1/accounts/{account_path}/statuses"),
            &parameters,
            "min_id",
            first_id,
            &[
                "limit",
                "pinned",
                "tagged",
                "only_media",
                "exclude_replies",
                "exclude_reblogs",
            ],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn account_followers(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    uri: Uri,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    account_follows(
        state,
        rack,
        uri,
        query,
        headers,
        FollowCollectionKind::Followers,
    )
    .await
}

async fn account_following(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    uri: Uri,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    account_follows(
        state,
        rack,
        uri,
        query,
        headers,
        FollowCollectionKind::Following,
    )
    .await
}

async fn account_follows(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    kind: FollowCollectionKind,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((account_path, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let loader = state.loader(viewer);
    match loader.account(account_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    match loader.follow_collection_hidden(account_id).await {
        Ok(true) => return json_response(StatusCode::OK, b"[]".to_vec()),
        Ok(false) => {}
        Err(_) => return internal_error(),
    }
    let parameters = parameters(query.as_deref());
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = FollowCollectionOptions {
        max_id,
        since_id,
        limit: match limit_parameter(&rack, 40, 80) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let Ok(page) = loader.follow_collection(account_id, kind, &options).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let accounts = page
        .accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>();
    let mut response = match accounts
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    };
    let route = match kind {
        FollowCollectionKind::Followers => format!("api/v1/accounts/{account_path}/followers"),
        FollowCollectionKind::Following => format!("api/v1/accounts/{account_path}/following"),
    };
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.accounts.len() == limit)
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "since_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn status_history(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_scope_owner(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let history = match state.loader(viewer).status_history(status_id).await {
        Ok(Some(history)) => history,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let values = history
        .iter()
        .map(|edit| state.serializer().status_edit(edit))
        .collect::<Result<Vec<_>, _>>();
    match values
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn status_source(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match required_scope_owner(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let status = match state.loader(viewer).authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    match serde_json::to_vec(&state.serializer().status_source(&status)) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn favourited_by(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_account_association(state, rack, query, uri, headers, false).await
}

async fn reblogged_by(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_account_association(state, rack, query, uri, headers, true).await
}

async fn status_account_association(
    state: WebState,
    rack: RackParameters,
    query: Option<String>,
    uri: Uri,
    headers: HeaderMap,
    reblogs: bool,
) -> Response<Body> {
    let viewer = match optional_scope_owner(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((status_path, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let parameters = parameters(query.as_deref());
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = FollowCollectionOptions {
        max_id,
        since_id,
        limit: match limit_parameter(&rack, 40, 80) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let page = match if reblogs {
        state.loader(viewer).reblogged_by(status_id, &options).await
    } else {
        state
            .loader(viewer)
            .favourited_by(status_id, &options)
            .await
    } {
        Ok(Some(page)) => page,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let serializer = state.serializer();
    let accounts = page
        .accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>();
    let mut response = match accounts
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    };
    let suffix = if reblogs {
        "reblogged_by"
    } else {
        "favourited_by"
    };
    let route = format!("api/v1/statuses/{status_path}/{suffix}");
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.accounts.len() == limit)
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "since_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn status_show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let status = match state.loader(viewer).authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status(&status, StatusShape::Full)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn status_context(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let context = match state.loader(viewer).status_context(status_id).await {
        Ok(Some(context)) => context,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status_context(&context)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn public_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let parameters = parameters(query.as_deref());
    let requested_feed = requested_feed_options(&rack);
    let (viewer, access) = match timeline_viewer(
        &state,
        &headers,
        READ_STATUSES,
        &requested_feed,
        FeedKind::Public,
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return response,
    };
    let mut options = match timeline_options(&rack) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    if !apply_feed_access(&mut options, access) {
        return timeline_response(
            &state,
            "api/v1/timelines/public",
            &parameters,
            &["local", "remote", "limit", "only_media"],
            &[],
        );
    }
    let Ok(statuses) = state.loader(viewer).public_timeline(&options).await else {
        return internal_error();
    };
    timeline_response(
        &state,
        "api/v1/timelines/public",
        &parameters,
        &["local", "remote", "limit", "only_media"],
        &statuses,
    )
}

async fn tag_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let parameters = parameters(query.as_deref());
    let requested_feed = requested_feed_options(&rack);
    let (viewer, access) = match timeline_viewer(
        &state,
        &headers,
        READ_STATUSES,
        &requested_feed,
        FeedKind::Topic,
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return response,
    };
    let Some(hashtag) = uri_path_segment(&uri, 5) else {
        return not_found();
    };
    match state.loader(viewer).tag_exists(&hashtag).await {
        Ok(false) => {
            return timeline_response(
                &state,
                &format!("api/v1/timelines/tag/{hashtag}"),
                &parameters,
                &["local", "limit", "only_media"],
                &[],
            );
        }
        Ok(true) => {}
        Err(_) => return internal_error(),
    }
    let mut page = match timeline_options(&rack) {
        Ok(page) => page,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    if !apply_feed_access(&mut page, access) {
        return timeline_response(
            &state,
            &format!("api/v1/timelines/tag/{hashtag}"),
            &parameters,
            &["local", "limit", "only_media"],
            &[],
        );
    }
    let options = TagTimelineOptions {
        page,
        any: rack_array_values(&rack, "any"),
        all: rack_array_values(&rack, "all"),
        none: rack_array_values(&rack, "none"),
    };
    let Ok(statuses) = state.loader(viewer).tag_timeline(&hashtag, &options).await else {
        return internal_error();
    };
    timeline_response(
        &state,
        &format!("api/v1/timelines/tag/{hashtag}"),
        &parameters,
        &["local", "limit", "only_media"],
        &statuses,
    )
}

async fn home_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_STATUSES).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let parameters = parameters(query.as_deref());
    let options = match timeline_options(&rack) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let Ok(statuses) = state
        .loader(Some(owner))
        .home_timeline(owner, &options)
        .await
    else {
        return internal_error();
    };
    timeline_response(
        &state,
        "api/v1/timelines/home",
        &parameters,
        &["local", "limit"],
        &statuses,
    )
}

async fn list_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_LISTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((list_path, list_id)) = uri_path_id(&uri, 5) else {
        return not_found();
    };
    match state
        .loader(Some(owner))
        .owned_list_exists(owner, list_id)
        .await
    {
        Ok(false) => return record_not_found(),
        Ok(true) => {}
        Err(_) => return internal_error(),
    }
    let parameters = parameters(query.as_deref());
    let options = match timeline_options(&rack) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let statuses = match state
        .loader(Some(owner))
        .list_timeline(owner, list_id, &options)
        .await
    {
        Ok(Some(statuses)) => statuses,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    timeline_response(
        &state,
        &format!("api/v1/timelines/list/{list_path}"),
        &parameters,
        &["limit"],
        &statuses,
    )
}

async fn favourites(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    saved_statuses(state, rack, query, headers, SavedStatusKind::Favourites).await
}

async fn bookmarks(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    saved_statuses(state, rack, query, headers, SavedStatusKind::Bookmarks).await
}

async fn saved_statuses(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    kind: SavedStatusKind,
) -> Response<Body> {
    let (scopes, route) = match kind {
        SavedStatusKind::Favourites => (READ_FAVOURITES, "api/v1/favourites"),
        SavedStatusKind::Bookmarks => (READ_BOOKMARKS, "api/v1/bookmarks"),
    };
    let owner = match required_viewer(&state, &headers, scopes).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let parameters = parameters(query.as_deref());
    let (max_id, min_id, since_id) = match cursor_triplet(&rack) {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = SavedStatusesOptions {
        max_id,
        min_id,
        since_id,
        limit: match limit_parameter(&rack, 20, 40) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let Ok(page) = state
        .loader(Some(owner))
        .saved_statuses(owner, kind, &options)
        .await
    else {
        return internal_error();
    };
    let mut response = statuses_response(&state, &page.statuses);
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.statuses.len() == limit)
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            route,
            &parameters,
            "min_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn blocks(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    account_list(state, rack, query, headers, AccountListKind::Blocks).await
}

async fn mutes(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    account_list(state, rack, query, headers, AccountListKind::Mutes).await
}

async fn account_list(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    kind: AccountListKind,
) -> Response<Body> {
    let (scopes, route) = match kind {
        AccountListKind::Blocks => (READ_BLOCKS, "api/v1/blocks"),
        AccountListKind::Mutes => (READ_MUTES, "api/v1/mutes"),
    };
    let owner = match required_viewer(&state, &headers, scopes).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let parameters = parameters(query.as_deref());
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = AccountListOptions {
        max_id,
        since_id,
        limit: match limit_parameter(&rack, 40, 80) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let Ok(page) = state
        .loader(Some(owner))
        .account_list(owner, kind, &options)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = page.entries.iter().map(|entry| match kind {
        AccountListKind::Blocks => serializer
            .account(&entry.account)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok()),
        AccountListKind::Mutes => serializer
            .muted_account(&entry.account, entry.mute_expires_at)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok()),
    });
    let mut response = match values
        .collect::<Option<Vec<_>>>()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => return internal_error(),
    };
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.entries.len() == limit)
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            route,
            &parameters,
            "since_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

fn timeline_response(
    state: &WebState,
    route: &str,
    parameters: &[(String, String)],
    preserved_names: &[&str],
    statuses: &[crate::mastodon::rest::StatusProjection],
) -> Response<Body> {
    let mut response = statuses_response(state, statuses);
    let mut links = Vec::new();
    if let Some(last_id) = statuses.last().map(|status| status.id)
        && let Some(url) =
            pagination_url(state, route, parameters, "max_id", last_id, preserved_names)
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_id) = statuses.first().map(|status| status.id)
        && let Some(url) = pagination_url(
            state,
            route,
            parameters,
            "min_id",
            first_id,
            preserved_names,
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

fn statuses_response(
    state: &WebState,
    statuses: &[crate::mastodon::rest::StatusProjection],
) -> Response<Body> {
    let serializer = state.serializer();
    let values = statuses
        .iter()
        .map(|status| serializer.status(status, StatusShape::Full))
        .collect::<Result<Vec<_>, _>>();
    match values
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn optional_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<Option<i64>, Response<Body>> {
    optional_viewer_owner(state, headers, scopes)
        .await
        .map(|owner| owner.map(OAuthResourceOwner::account_id))
}

async fn optional_viewer_owner(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<Option<OAuthResourceOwner>, Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Ok(None);
    }
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => Ok(authenticated.resource_owner()),
        Err(OAuthAuthenticationError::OAuth(
            OAuthError::Unauthenticated
            | OAuthError::InvalidToken(crate::mastodon::InvalidTokenReason::Unknown),
        )) => Ok(None),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn required_viewer_owner(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<OAuthResourceOwner, Response<Body>> {
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => authenticated
            .require_user()
            .map_err(|error| error.into_http_response().map(Body::from)),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn required_scope_owner(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<Option<i64>, Response<Body>> {
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => Ok(authenticated
            .resource_owner()
            .map(OAuthResourceOwner::account_id)),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn optional_scope_owner(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<Option<i64>, Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Ok(None);
    }
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => Ok(authenticated
            .resource_owner()
            .map(OAuthResourceOwner::account_id)),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn required_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<i64, Response<Body>> {
    required_viewer_owner(state, headers, scopes)
        .await
        .map(OAuthResourceOwner::account_id)
}

fn parameters(query: Option<&str>) -> Vec<(String, String)> {
    query
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn path_id(value: &str) -> Option<i64> {
    parse_path_id(value, true)
}

fn activitypub_path_id(value: &str) -> Option<i64> {
    if value != trim_ascii_start(value) || value.starts_with('+') {
        return None;
    }
    path_id(value)
}

fn route_path_id(value: &str) -> Option<i64> {
    parse_path_id(value, false)
}

fn parse_path_id(value: &str, require_full: bool) -> Option<i64> {
    let value = trim_ascii_start(value);
    let (negative, digits) = value.strip_prefix('-').map_or_else(
        || (false, value.strip_prefix('+').unwrap_or(value)),
        |value| (true, value),
    );
    let digits = if require_full {
        if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
            return None;
        }
        digits
    } else {
        &digits[..digits.bytes().take_while(u8::is_ascii_digit).count()]
    };
    if digits.is_empty() {
        return None;
    }
    let mut value = 0_i64;
    for digit in digits.bytes() {
        let digit = i64::from(digit - b'0');
        value = if negative {
            value.checked_mul(10)?.checked_sub(digit)?
        } else {
            value.checked_mul(10)?.checked_add(digit)?
        };
    }
    Some(value)
}

fn uri_path_id(uri: &Uri, segment: usize) -> Option<(String, i64)> {
    let path = uri.path().split('/').nth(segment)?;
    let decoded = percent_decode_str(path).decode_utf8().ok()?;
    let id = route_path_id(&decoded)?;
    Some((canonical_path_segment(&decoded), id))
}

fn canonical_path_segment(value: &str) -> String {
    utf8_percent_encode(value, RAILS_PATH_SEGMENT).to_string()
}

fn uri_path_segment(uri: &Uri, segment: usize) -> Option<String> {
    percent_decode_str(uri.path().split('/').nth(segment)?)
        .decode_utf8()
        .ok()
        .filter(|value| !value.is_empty())
        .map(std::borrow::Cow::into_owned)
}

fn string_parameter<'a>(parameters: &'a [(String, String)], name: &str) -> Option<&'a str> {
    parameters
        .iter()
        .rfind(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

fn rack_array_values(parameters: &RackParameters, name: &str) -> Vec<String> {
    match parameters.get(name) {
        Some(RackValue::Scalar(value)) => vec![value.clone()],
        Some(RackValue::Number(value)) => vec![value.to_string()],
        Some(RackValue::Boolean(value)) => vec![value.to_string()],
        Some(RackValue::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                RackValue::Null => Some(String::new()),
                RackValue::Scalar(value) => Some(value.clone()),
                RackValue::Number(value) => Some(ruby_json_number(value)),
                RackValue::Boolean(value) => Some(value.to_string()),
                RackValue::Array(_) | RackValue::Object(_) => None,
            })
            .collect(),
        None | Some(RackValue::Null | RackValue::Object(_)) => Vec::new(),
    }
}

fn tagged_parameter(parameters: &RackParameters) -> Option<String> {
    match parameters.get("tagged") {
        None | Some(RackValue::Null) => None,
        Some(RackValue::Scalar(value)) => (!value.trim().is_empty()).then(|| value.clone()),
        Some(RackValue::Number(value)) => Some(ruby_json_number(value)),
        Some(RackValue::Boolean(value)) => Some(value.to_string()),
        Some(RackValue::Array(values)) => values.iter().find_map(|value| match value {
            RackValue::Scalar(value) if !value.trim().is_empty() => Some(value.clone()),
            RackValue::Number(value) => Some(ruby_json_number(value)),
            RackValue::Boolean(value) => Some(value.to_string()),
            RackValue::Null | RackValue::Scalar(_) | RackValue::Array(_) | RackValue::Object(_) => {
                None
            }
        }),
        Some(RackValue::Object(_)) => Some(String::new()),
    }
}

fn relationship_ids(parameters: &RackParameters) -> Result<Vec<i64>, ()> {
    match parameters.get("id") {
        None | Some(RackValue::Null) => Ok(Vec::new()),
        Some(RackValue::Scalar(value)) => Ok(vec![ruby_integer(value)]),
        Some(RackValue::Number(value)) => Ok(vec![json_number_integer(value)?]),
        Some(RackValue::Boolean(_) | RackValue::Object(_)) => Err(()),
        Some(RackValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                RackValue::Null => Ok(0),
                RackValue::Scalar(value) => Ok(ruby_integer(value)),
                RackValue::Number(value) => json_number_integer(value),
                RackValue::Boolean(_) | RackValue::Array(_) | RackValue::Object(_) => Err(()),
            })
            .collect(),
    }
}

fn timeline_options(parameters: &RackParameters) -> Result<TimelineOptions, CursorParameterError> {
    let (max_id, min_id, since_id) = cursor_triplet(parameters)?;
    Ok(TimelineOptions {
        max_id,
        min_id,
        since_id,
        limit: limit_parameter(parameters, 20, 40)
            .map_err(|()| CursorParameterError::InvalidScalar)?,
        local: boolean_parameter(parameters, "local"),
        remote: boolean_parameter(parameters, "remote"),
        only_media: boolean_parameter(parameters, "only_media"),
    })
}

fn requested_feed_options(parameters: &RackParameters) -> TimelineOptions {
    TimelineOptions {
        local: boolean_parameter(parameters, "local"),
        remote: boolean_parameter(parameters, "remote"),
        ..TimelineOptions::default()
    }
}

#[derive(Clone, Copy)]
enum FeedKind {
    Public,
    Topic,
}

#[derive(Clone, Copy)]
struct FeedAccess {
    local: bool,
    remote: bool,
}

async fn timeline_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
    options: &TimelineOptions,
    kind: FeedKind,
) -> Result<(Option<i64>, FeedAccess), Response<Body>> {
    let settings = state
        .repository
        .settings()
        .await
        .map_err(|_| internal_error())?;
    let setting = |name: &str| {
        settings
            .iter()
            .find(|setting| setting.var == name)
            .and_then(|setting| setting.value.as_ref())
            .and_then(|value| yaml_scalar(value.raw()))
            .unwrap_or_else(|| "public".to_owned())
    };
    let (local, remote) = match kind {
        FeedKind::Public => (
            setting("local_live_feed_access"),
            setting("remote_live_feed_access"),
        ),
        FeedKind::Topic => (
            setting("local_topic_feed_access"),
            setting("remote_topic_feed_access"),
        ),
    };
    let requires_user = if options.local {
        local != "public"
    } else if options.remote {
        remote != "public"
    } else {
        local != "public" || remote != "public"
    };
    let owner = if requires_user {
        Some(required_timeline_viewer(state, headers, scopes).await?)
    } else {
        optional_viewer_owner(state, headers, scopes).await?
    };
    let viewer = owner.map(OAuthResourceOwner::account_id);
    let can_view_disabled = match owner {
        Some(owner) => state
            .repository
            .user_can_view_feeds(owner.user_id(), owner.account_id())
            .await
            .map_err(|_| internal_error())?,
        None => false,
    };
    let allowed = |setting: &str| match setting {
        "public" => true,
        "authenticated" => viewer.is_some(),
        "disabled" => can_view_disabled,
        _ => false,
    };
    Ok((
        viewer,
        FeedAccess {
            local: allowed(&local),
            remote: allowed(&remote),
        },
    ))
}

fn apply_feed_access(options: &mut TimelineOptions, access: FeedAccess) -> bool {
    let requested_local = !options.remote || options.local;
    let requested_remote = !options.local || options.remote;
    let local = requested_local && access.local;
    let remote = requested_remote && access.remote;
    options.local = local && !remote;
    options.remote = remote && !local;
    local || remote
}

async fn required_timeline_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<OAuthResourceOwner, Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Err(OAuthError::UserRequired
            .into_http_response()
            .map(Body::from));
    }
    required_viewer_owner(state, headers, scopes).await
}

fn yaml_scalar(raw: &str) -> Option<String> {
    let value = raw.trim();
    let value = value.strip_prefix("---").unwrap_or(value).trim();
    if value.is_empty() || matches!(value, "null" | "~") {
        return None;
    }
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str(value).ok();
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return Some(value[1..value.len() - 1].replace("''", "'"));
    }
    Some(value.to_owned())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorParameterError {
    InvalidScalar,
    Overflow,
}

type CursorTriplet = (Option<i64>, Option<i64>, Option<i64>);

fn integer_parameter(
    parameters: &RackParameters,
    name: &str,
) -> Result<Option<i64>, CursorParameterError> {
    let empty_bound = || {
        if name == "max_id" { i64::MIN } else { i64::MAX }
    };
    let value = match parameters.get(name) {
        None | Some(RackValue::Null) => return Ok(None),
        Some(RackValue::Scalar(value)) if value.trim().is_empty() => return Ok(None),
        Some(RackValue::Scalar(value)) => value,
        Some(RackValue::Number(value)) => {
            return json_number_integer(value)
                .map(Some)
                .map_err(|()| CursorParameterError::Overflow);
        }
        Some(RackValue::Boolean(_)) => return Err(CursorParameterError::InvalidScalar),
        Some(RackValue::Array(_) | RackValue::Object(_)) => return Ok(Some(empty_bound())),
    };
    checked_integer_prefix(value)
        .map_err(|()| CursorParameterError::Overflow)
        .map(|value| Some(value.unwrap_or_else(empty_bound)))
}

fn cursor_pair(
    parameters: &RackParameters,
    first: &str,
    second: &str,
) -> Result<(Option<i64>, Option<i64>), CursorParameterError> {
    Ok((
        integer_parameter(parameters, first)?,
        integer_parameter(parameters, second)?,
    ))
}

fn cursor_triplet(parameters: &RackParameters) -> Result<CursorTriplet, CursorParameterError> {
    Ok((
        integer_parameter(parameters, "max_id")?,
        integer_parameter(parameters, "min_id")?,
        integer_parameter(parameters, "since_id")?,
    ))
}

fn cursor_parameter_error(_headers: &HeaderMap, error: CursorParameterError) -> Response<Body> {
    match error {
        CursorParameterError::InvalidScalar | CursorParameterError::Overflow => {
            framework_internal_error()
        }
    }
}

fn boolean_parameter(parameters: &RackParameters, name: &str) -> bool {
    match parameters.get(name) {
        None | Some(RackValue::Null) => false,
        Some(RackValue::Scalar(value)) => {
            !value.is_empty()
                && !matches!(
                    value.as_str(),
                    "0" | "f" | "F" | "false" | "FALSE" | "off" | "OFF"
                )
        }
        Some(RackValue::Number(_) | RackValue::Array(_) | RackValue::Object(_)) => true,
        Some(RackValue::Boolean(value)) => *value,
    }
}

fn limit_parameter(parameters: &RackParameters, default: i64, maximum: i64) -> Result<i64, ()> {
    match parameters.get("limit") {
        None | Some(RackValue::Null) => Ok(default),
        Some(RackValue::Scalar(value)) => Ok(ruby_integer(value).saturating_abs().min(maximum)),
        Some(RackValue::Number(value)) => json_number_limit(value, maximum),
        Some(RackValue::Boolean(_) | RackValue::Array(_) | RackValue::Object(_)) => Err(()),
    }
}

fn ruby_integer(value: &str) -> i64 {
    let value = trim_ascii_start(value);
    let bytes = value.as_bytes();
    let (negative, start) = match bytes.first() {
        Some(b'-') => (true, 1),
        Some(b'+') => (false, 1),
        _ => (false, 0),
    };
    let magnitude = bytes[start..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .fold(0_i64, |number, byte| {
            number
                .saturating_mul(10)
                .saturating_add(i64::from(*byte - b'0'))
        });
    if negative {
        magnitude.saturating_neg()
    } else {
        magnitude
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn json_number_integer(value: &serde_json::Number) -> Result<i64, ()> {
    value
        .as_i64()
        .map(Ok)
        .or_else(|| {
            value
                .as_u64()
                .map(|value| i64::try_from(value).map_err(|_| ()))
        })
        .unwrap_or_else(|| {
            let value = value.as_f64().ok_or(())?;
            if value < i64::MIN as f64 || value >= i64::MAX as f64 {
                Err(())
            } else {
                Ok(value as i64)
            }
        })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn json_number_limit(value: &serde_json::Number, maximum: i64) -> Result<i64, ()> {
    if value.as_i64().is_none() && value.as_u64().is_none() && value.as_f64().is_none() {
        return Err(());
    }
    if value
        .as_i64()
        .is_some_and(|value| value.unsigned_abs() >= maximum as u64)
        || value.as_u64().is_some_and(|value| value >= maximum as u64)
        || value
            .as_f64()
            .is_some_and(|value| value.abs() >= maximum as f64)
    {
        Ok(maximum)
    } else {
        Ok(value.as_f64().unwrap_or_default().abs() as i64)
    }
}

fn ruby_json_number(value: &serde_json::Number) -> String {
    if value.as_i64().is_some() || value.as_u64().is_some() {
        return value.to_string();
    }
    value.as_f64().map_or_else(
        || value.to_string(),
        |value| {
            let absolute = value.abs();
            if absolute >= 1e15 || (absolute != 0.0 && absolute < 1e-4) {
                let scientific = format!("{value:e}");
                let (mantissa, exponent) = scientific
                    .split_once('e')
                    .expect("scientific float contains exponent");
                let mantissa = if mantissa.contains('.') {
                    mantissa.to_owned()
                } else {
                    format!("{mantissa}.0")
                };
                let (sign, digits) = exponent
                    .strip_prefix('+')
                    .map_or_else(
                        || exponent.strip_prefix('-').map(|digits| ('-', digits)),
                        |digits| Some(('+', digits)),
                    )
                    .unwrap_or(('+', exponent));
                let exponent = if digits.len() < 2 {
                    format!("{sign}0{digits}")
                } else {
                    format!("{sign}{digits}")
                };
                format!("{mantissa}e{exponent}")
            } else if value.fract() == 0.0 {
                format!("{value:.1}")
            } else {
                value.to_string()
            }
        },
    )
}

fn checked_integer_prefix(value: &str) -> Result<Option<i64>, ()> {
    let value = trim_ascii_start(value);
    let (negative, digits) = value.strip_prefix('-').map_or_else(
        || (false, value.strip_prefix('+').unwrap_or(value)),
        |value| (true, value),
    );
    let mut number = 0_i64;
    let mut found = false;
    for digit in digits.bytes().take_while(u8::is_ascii_digit) {
        found = true;
        let digit = i64::from(digit - b'0');
        number = if negative {
            number
                .checked_mul(10)
                .and_then(|value| value.checked_sub(digit))
        } else {
            number
                .checked_mul(10)
                .and_then(|value| value.checked_add(digit))
        }
        .ok_or(())?;
    }
    Ok(found.then_some(number))
}

fn trim_ascii_start(value: &str) -> &str {
    value.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c'])
}

fn pagination_url(
    state: &WebState,
    route: &str,
    parameters: &[(String, String)],
    cursor_name: &str,
    cursor: i64,
    preserved_names: &[&str],
) -> Option<String> {
    let mut url = state.origin.join(route).ok()?;
    {
        let cursor = cursor.to_string();
        let mut parameters = preserved_names
            .iter()
            .filter_map(|name| string_parameter(parameters, name).map(|value| (*name, value)))
            .collect::<Vec<_>>();
        parameters.push((cursor_name, &cursor));
        parameters.sort_by_key(|(name, _)| *name);
        let mut pairs = url.query_pairs_mut();
        for (name, value) in parameters {
            pairs.append_pair(name, value);
        }
    }
    Some(url.to_string())
}

fn set_link_header(response: &mut Response<Body>, links: &[String]) {
    if !links.is_empty()
        && let Ok(value) = HeaderValue::from_str(&links.join(", "))
    {
        response.headers_mut().insert("link", value);
    }
}

fn json_response(status: StatusCode, body: Vec<u8>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(body))
        .expect("static response headers are valid")
}

fn error_response(status: StatusCode, message: &str) -> Response<Body> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message }))
        .expect("an error envelope with one string is serializable");
    json_response(status, body)
}

fn record_not_found() -> Response<Body> {
    error_response(StatusCode::NOT_FOUND, "Record not found")
}

fn not_found() -> Response<Body> {
    error_response(StatusCode::NOT_FOUND, "Not Found")
}

fn api_not_found() -> Response<Body> {
    not_found()
}

fn framework_not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(CONTENT_TYPE, "application/json; charset=UTF-8")
        .body(Body::from(r#"{"status":404,"error":"Not Found"}"#))
        .expect("static framework 404 response is valid")
}

fn internal_error() -> Response<Body> {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
}

fn framework_internal_error() -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(CONTENT_TYPE, "application/json; charset=UTF-8")
        .header(VARY, "Origin")
        .header(FRAMEWORK_ERROR_HEADER, "1")
        .body(Body::from(
            r#"{"status":500,"error":"Internal Server Error"}"#,
        ))
        .expect("static framework error response is valid")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use axum::http::header::{
        ACCEPT, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
        ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_REQUEST_HEADERS,
        ACCESS_CONTROL_REQUEST_METHOD, ORIGIN,
    };

    use super::*;

    #[test]
    fn api_route_ids_preserve_rails_prefix_casting_without_relaxing_other_ids() {
        assert_eq!(
            route_path_id("116844606259201001%3Fjunk"),
            Some(116_844_606_259_201_001)
        );
        assert_eq!(
            route_path_id("116844606259201001?junk"),
            Some(116_844_606_259_201_001)
        );
        assert_eq!(
            route_path_id("+116844606259201001"),
            Some(116_844_606_259_201_001)
        );
        assert_eq!(route_path_id("not-an-id"), None);
        assert_eq!(path_id("116844606259201001?junk"), None);
        assert_eq!(
            activitypub_path_id("-116844606259201001"),
            Some(-116_844_606_259_201_001)
        );
        assert_eq!(activitypub_path_id("+116844606259201001"), None);
        assert_eq!(activitypub_path_id(" 116844606259201001"), None);
    }

    #[test]
    fn activitypub_negotiation_honors_media_parameters_and_quality() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/html, application/activity+json; q=0"),
        );
        assert!(!accepts_activitypub(&headers));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json; charset=utf-8; q=0.5"),
        );
        assert!(accepts_activitypub(&headers));
        headers.insert(ACCEPT, HeaderValue::from_static("application/ld+json;Q=0"));
        assert!(!accepts_activitypub(&headers));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/activity+json;q=0.1, text/html;q=0.9"),
        );
        assert!(!accepts_activitypub(&headers));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/html;q=0.9, application/activity+json;q=1"),
        );
        assert!(accepts_activitypub(&headers));
    }

    #[test]
    fn federation_authority_matching_preserves_explicit_ports() {
        let origin = Url::parse("https://example.test:8443/").expect("valid origin");
        assert_eq!(
            federation_url_authority(&origin).as_deref(),
            Some("example.test:8443")
        );
    }

    #[test]
    fn forwarded_metadata_is_accepted_only_from_trusted_peers() {
        let trusted = ["10.0.0.0/8".parse().unwrap()];
        let headers = HeaderMap::from_iter([
            (
                "x-forwarded-for".parse().unwrap(),
                "198.51.100.8, 10.1.2.3".parse().unwrap(),
            ),
            (
                "x-forwarded-proto".parse().unwrap(),
                "https".parse().unwrap(),
            ),
            (
                "x-forwarded-host".parse().unwrap(),
                "social.example".parse().unwrap(),
            ),
        ]);
        let peer = "10.1.2.3:4321".parse().unwrap();
        let metadata = request_metadata(peer, &headers, &trusted).unwrap();
        assert_eq!(
            metadata.client_ip,
            "198.51.100.8".parse::<IpAddr>().unwrap()
        );
        assert_eq!(metadata.scheme.as_deref(), Some("https"));
        assert_eq!(metadata.host.as_deref(), Some("social.example"));

        let untrusted =
            request_metadata("203.0.113.7:1234".parse().unwrap(), &headers, &trusted).unwrap();
        assert_eq!(
            untrusted.client_ip,
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        assert!(untrusted.scheme.is_none());
        assert!(untrusted.host.is_none());

        let unconfigured = request_metadata(peer, &headers, &[]).unwrap();
        assert_eq!(unconfigured.client_ip, peer.ip());
        assert!(unconfigured.scheme.is_none());
        assert!(unconfigured.host.is_none());
    }

    #[test]
    fn malformed_forwarding_from_a_trusted_peer_fails_closed() {
        let trusted = ["10.0.0.0/8".parse().unwrap()];
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        assert!(request_metadata("10.1.2.3:1".parse().unwrap(), &headers, &trusted).is_err());
    }

    #[test]
    fn request_hosts_are_compared_as_exact_authorities() {
        let allowed = ["social.example".to_owned(), "web.example:8443".to_owned()];
        assert!(
            allowed
                .iter()
                .any(|host| host.eq_ignore_ascii_case("SOCIAL.EXAMPLE"))
        );
        assert!(allowed.iter().any(|host| host == "web.example:8443"));
        assert!(!allowed.iter().any(|host| host == "web.example"));
        assert!(!allowed.iter().any(|host| host == "social.example.evil"));
    }

    #[test]
    fn api_route_inventory_is_unique_and_declares_protocol_contracts() {
        assert_eq!(API_ROUTE_INVENTORY.len(), 32);
        assert_eq!(REST_BODY_LIMIT_BYTES, 103_809_024);
        assert_eq!(
            API_ROUTE_INVENTORY
                .iter()
                .map(|route| route.path)
                .collect::<BTreeSet<_>>()
                .len(),
            API_ROUTE_INVENTORY.len()
        );
        assert_eq!(
            api_route("/api/v1/instance/translation_languages")
                .expect("disabled translation response is inventoried")
                .support,
            ApiRouteSupport::DisabledResponse
        );
        assert_eq!(
            api_route("/api/v1/timelines/list/9001")
                .expect("dynamic list route matches")
                .pagination,
            PaginationContract::StatusId
        );
        assert_eq!(
            api_route("/api/v1/accounts/relationships")
                .expect("relationships are inventoried")
                .authentication,
            ApiAuthentication::Required(READ_FOLLOWS.as_slice())
        );
        assert!(
            API_ROUTE_INVENTORY
                .iter()
                .all(|route| route.method == ApiMethod::Get)
        );
        assert!(api_route("/api/v1/markers").is_some());
    }

    #[test]
    fn cors_preflight_is_limited_to_inventoried_routes() {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
        headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
        headers.insert(
            ACCESS_CONTROL_REQUEST_HEADERS,
            HeaderValue::from_static("authorization, x-client"),
        );
        let response = cors_preflight_response("/api/v1/timelines/home", &headers)
            .expect("implemented API route accepts preflight");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_METHODS],
            "POST, PUT, DELETE, GET, PATCH, OPTIONS"
        );
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_HEADERS],
            "authorization, x-client"
        );
        assert!(!response.headers().contains_key(VARY));
        assert!(cors_preflight_response("/api/v1/markers", &headers).is_some());
        assert!(cors_preflight_response("/api/v1/accounts/search", &headers).is_some());
        assert!(cors_preflight_response("/api/v1/accounts/familiar_followers", &headers).is_none());
        assert!(cors_preflight_response("/api/v1/accounts/search/statuses", &headers).is_some());
        headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("POST"),
        );
        assert!(cors_preflight_response("/api/v1/timelines/home", &headers).is_some());
    }

    #[test]
    fn response_finalization_applies_cache_vary_and_cors_by_route() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
        let instance = finalize_api_response(
            "/api/v2/instance",
            &request_headers,
            json_response(StatusCode::OK, b"{}".to_vec()),
        );
        assert_eq!(
            instance.headers()[CACHE_CONTROL],
            "max-age=300, public, stale-while-revalidate=30, stale-if-error=86400"
        );
        assert_eq!(instance.headers()[VARY], "Accept, Origin");
        assert_eq!(instance.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(
            instance.headers()[ACCESS_CONTROL_EXPOSE_HEADERS]
                .to_str()
                .expect("static CORS headers are text")
                .contains("Link")
        );

        request_headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer fixture"));
        let account = finalize_api_response(
            "/api/v1/accounts/1",
            &request_headers,
            json_response(StatusCode::OK, b"{}".to_vec()),
        );
        assert_eq!(account.headers()[CACHE_CONTROL], "private, no-store");
        assert_eq!(account.headers()[VARY], "Authorization, Origin");
    }

    #[test]
    fn api_fallbacks_use_the_mastodon_json_envelope() {
        let response = api_not_found();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn paperclip_ranges_cover_rack_media_cases() {
        assert_eq!(
            paperclip_ranges(Some("bytes=2-4"), 10),
            RangeSelection::Ranges(vec![ByteRange { start: 2, end: 4 }])
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=7-"), 10),
            RangeSelection::Ranges(vec![ByteRange { start: 7, end: 9 }])
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=-3"), 10),
            RangeSelection::Ranges(vec![ByteRange { start: 7, end: 9 }])
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=0-1,8-9"), 10),
            RangeSelection::Ranges(vec![
                ByteRange { start: 0, end: 1 },
                ByteRange { start: 8, end: 9 },
            ])
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=10-"), 10),
            RangeSelection::Unsatisfiable
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=8-2"), 10),
            RangeSelection::Full
        );
        assert_eq!(
            paperclip_ranges(Some("Bytes=0-1"), 10),
            RangeSelection::Full
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=18446744073709551616-"), 10),
            RangeSelection::Unsatisfiable
        );
        assert_eq!(paperclip_ranges(Some("bytes=0-1"), 0), RangeSelection::Full);
    }

    #[test]
    fn absolute_media_route_paths_are_normalized_once() {
        assert_eq!(
            media_route("https://media.example/assets"),
            ("/assets".to_owned(), Some("media.example".to_owned()))
        );
        assert_eq!(
            media_route("https://media.example/assets/"),
            ("/assets".to_owned(), Some("media.example".to_owned()))
        );
        assert_eq!(
            media_route("https://media.example/"),
            (String::new(), Some("media.example".to_owned()))
        );
    }

    #[tokio::test]
    async fn request_body_limit_rejects_oversized_bodies() {
        let request = Request::builder()
            .uri("/api/v2/instance")
            .body(Body::from(vec![0_u8; 5]))
            .expect("static request is valid");
        let response = bounded_request(request, 4)
            .await
            .expect_err("oversized request is rejected");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "application/json; charset=utf-8"
        );

        let request = Request::builder()
            .uri("/api/v2/instance")
            .body(Body::from("body"))
            .expect("static request is valid");
        let request = bounded_request(request, 4)
            .await
            .expect("accepted body is replayed");
        let body = axum::body::to_bytes(request.into_body(), 4)
            .await
            .expect("replayed body is readable");
        assert_eq!(body, "body");
    }

    #[test]
    fn query_validation_rejects_bad_escapes_and_utf8() {
        assert!(valid_query("limit=2&tagged=fixturetag"));
        assert!(!valid_query("limit=%"));
        assert!(!valid_query("limit=%GG"));
        assert!(!valid_query("limit=%FF"));
    }

    #[test]
    fn cursor_parameters_reject_signed_bigint_overflow() {
        let positive = RackParameters::parse("max_id=9223372036854775808").unwrap();
        assert_eq!(
            integer_parameter(&positive, "max_id"),
            Err(CursorParameterError::Overflow)
        );
        let negative = RackParameters::parse("min_id=-9223372036854775809").unwrap();
        assert_eq!(
            integer_parameter(&negative, "min_id"),
            Err(CursorParameterError::Overflow)
        );
    }

    #[test]
    fn nonnumeric_cursor_parameters_produce_empty_bounds() {
        let invalid = RackParameters::parse("max_id=invalid&min_id=%2B&since_id=-").unwrap();
        assert_eq!(integer_parameter(&invalid, "max_id"), Ok(Some(i64::MIN)));
        assert_eq!(integer_parameter(&invalid, "min_id"), Ok(Some(i64::MAX)));
        assert_eq!(integer_parameter(&invalid, "since_id"), Ok(Some(i64::MAX)));
    }

    #[test]
    fn cursor_parameters_preserve_rack_scalar_shapes() {
        let nested = RackParameters::parse("max_id%5B%5D=1").unwrap();
        assert_eq!(integer_parameter(&nested, "max_id"), Ok(Some(i64::MIN)));
        assert!(RackParameters::parse("max_id=1&max_id%5B%5D=2").is_err());
    }

    #[test]
    fn consecutive_empty_brackets_match_rack_ordering() {
        let scalar_first = RackParameters::parse("a%5B%5D%5B%5D=1&a%5B%5D%5B%5D%5Bx%5D=2")
            .expect("mixed nested arrays parse");
        let Some(RackValue::Array(values)) = scalar_first.get("a") else {
            panic!("a is an array");
        };
        assert_eq!(values.len(), 2);
        assert!(matches!(
            values.as_slice(),
            [RackValue::Array(nested), RackValue::Object(object)]
                if matches!(nested.as_slice(), [RackValue::Scalar(value)] if value == "1")
                    && matches!(object.get("[]"), Some(RackValue::Object(child))
                        if matches!(child.get("x"), Some(RackValue::Scalar(value)) if value == "2"))
        ));

        let object_first = RackParameters::parse("a%5B%5D%5B%5D%5Bx%5D=1&a%5B%5D%5B%5D=2")
            .expect("terminal nested append after an object parses");
        let Some(RackValue::Array(values)) = object_first.get("a") else {
            panic!("a is an array");
        };
        assert!(matches!(
            values.as_slice(),
            [RackValue::Object(object)]
                if matches!(object.get("[]"), Some(RackValue::Object(child))
                    if matches!(child.get("x"), Some(RackValue::Scalar(value)) if value == "1"))
        ));
    }
}
