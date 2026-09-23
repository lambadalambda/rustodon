//! Bundled Mastodon web client: shell document, initial state, manifest and assets.

use super::*;

pub(super) async fn frontend_app(
    State(state): State<WebState>,
    request: Request,
) -> Response<Body> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return not_found();
    }
    let path = request.uri().path();
    if !is_frontend_path(path) {
        return not_found();
    }
    frontend_html_response(&state, path, request.headers()).await
}

pub(super) async fn web_fallback(
    State(state): State<WebState>,
    request: Request,
) -> Response<Body> {
    if is_frontend_path(request.uri().path()) {
        frontend_app(State(state), request).await
    } else {
        api_not_found()
    }
}

pub(super) async fn frontend_html_response(
    state: &WebState,
    path: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    let session = match request_cookie(headers, BROWSER_SESSION_COOKIE) {
        Some(session_id) => match state.repository.browser_session(session_id).await {
            Ok(session) => session,
            Err(_) => return internal_error(),
        },
        None => None,
    };
    let authenticated = if let Some(session) = session.as_ref() {
        if let Some(writer) = state.write_repository.as_ref()
            && writer
                .track_interactive_user(session.user_id)
                .await
                .is_err()
        {
            return internal_error();
        }
        let loader = state.loader(Some(session.account_id));
        let Ok(Some(credential)) = loader
            .credential_account(session.user_id, session.account_id)
            .await
        else {
            return internal_error();
        };
        let Ok(Some(preferences)) = loader
            .preferences(session.user_id, session.account_id)
            .await
        else {
            return internal_error();
        };
        let Ok(serialized) = state.serializer().credential_account(&credential) else {
            return internal_error();
        };
        let Ok(settings) = state
            .repository
            .web_settings(session.user_id, session.account_id)
            .await
        else {
            return internal_error();
        };
        Some(FrontendAuthenticatedState {
            account: serialized.account,
            access_token: session.access_token.as_str().to_owned(),
            account_id: session.account_id,
            preferences: state.serializer().preferences(&preferences),
            settings,
            role: serialized.role,
        })
    } else {
        None
    };
    let Ok(instance) = state.instance().await else {
        return internal_error();
    };
    let secure = state.origin.scheme() == "https";
    let (csrf_token, csrf_cookie) = browser_page_csrf(headers, secure, &state.csrf_signing_key);
    let csp_nonce = random_auth_token(32);
    let Some(serialized_instance) = state
        .serializer()
        .instance_v2(&instance)
        .ok()
        .and_then(|value| serde_json::to_value(value).ok())
    else {
        return internal_error();
    };
    let Some(document) = frontend_document(
        &state.frontend,
        &state.instance_runtime,
        path,
        Some(&instance),
        Some(serialized_instance),
        authenticated.as_ref(),
        &csrf_token,
        &csp_nonce,
    ) else {
        return internal_error();
    };
    let mut response = html_response(StatusCode::OK, document);
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_str(&frontend_content_security_policy(&csp_nonce))
            .expect("frontend CSP nonce is a valid header value"),
    );
    if let Some(cookie) = csrf_cookie {
        append_cookie(&mut response, &cookie);
    }
    response
}

pub(super) async fn frontend_manifest(State(state): State<WebState>) -> Response<Body> {
    let Ok(instance) = state.static_instance().await else {
        return internal_error();
    };
    let Some(value) = frontend_manifest_value(&state.frontend, &instance.title) else {
        return internal_error();
    };
    let mut response = json_response(
        StatusCode::OK,
        serde_json::to_vec(&value).expect("frontend manifest is serializable"),
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=180"),
    );
    response
}

pub(super) async fn frontend_service_worker(State(state): State<WebState>) -> Response<Body> {
    let mut response = frontend_file_response(
        &state.frontend,
        "packs/sw.js",
        "text/javascript; charset=utf-8",
        "no-cache",
    );
    if response.status().is_success() {
        response
            .headers_mut()
            .insert("service-worker-allowed", HeaderValue::from_static("/"));
    }
    response
}

pub(super) async fn frontend_favicon(State(state): State<WebState>) -> Response<Body> {
    let Some(path) = state.frontend.asset("icons/favicon-32x32.png") else {
        return not_found();
    };
    frontend_file_response(
        &state.frontend,
        &format!("packs/{}", path.file),
        "image/png",
        FRONTEND_CACHE,
    )
}

pub(super) async fn frontend_android_icon(State(state): State<WebState>) -> Response<Body> {
    let Some(path) = state.frontend.asset("icons/android-chrome-192x192.png") else {
        return not_found();
    };
    frontend_file_response(
        &state.frontend,
        &format!("packs/{}", path.file),
        "image/png",
        FRONTEND_CACHE,
    )
}

pub(super) fn frontend_file_response(
    frontend: &FrontendAssets,
    relative: &str,
    content_type: &str,
    cache_control: &str,
) -> Response<Body> {
    let Some(path) = frontend.path(relative) else {
        return not_found();
    };
    let Ok(body) = std::fs::read(path) else {
        return not_found();
    };
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_str(cache_control)
            .unwrap_or_else(|_| HeaderValue::from_static("no-cache")),
    );
    response
}

pub(super) async fn rustodon_stylesheet() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/css; charset=utf-8")
        .body(Body::from(RUSTODON_STYLESHEET))
        .expect("static stylesheet response headers are valid")
}

pub(super) fn frontend_asset_cache_control(target: &str) -> &'static str {
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    [(RUSTODON_STYLESHEET_URL, FRONTEND_CACHE)]
        .into_iter()
        .find(|(asset, _)| *asset == path)
        .map_or(FRONTEND_CACHE, |(_, policy)| policy)
}

pub(super) async fn frontend_asset_headers(request: Request, next: Next) -> Response<Body> {
    let target = request
        .uri()
        .path_and_query()
        .map_or(request.uri().path(), |target| target.as_str());
    let cache_control = frontend_asset_cache_control(target);
    let mut response = next.run(request).await;
    if response.status().is_success() {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    }
    response
}

pub(super) fn frontend_manifest_value(
    frontend: &FrontendAssets,
    title: &str,
) -> Option<serde_json::Value> {
    let icons = FRONTEND_ANDROID_ICON_SIZES
        .iter()
        .map(|size| {
            let source = format!("icons/android-chrome-{size}x{size}.png");
            let src = frontend.asset_url(&source)?;
            Some(serde_json::json!({
                "src": src,
                "sizes": format!("{size}x{size}"),
                "type": "image/png",
                "purpose": "any maskable",
            }))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(serde_json::json!({
        "instance": {
            "id": "/home",
            "name": title,
            "short_name": title,
            "icons": icons,
            "theme_color": "#191b22",
            "background_color": "#191b22",
            "display": "standalone",
            "start_url": "/",
            "scope": "/",
            "share_target": {
                "url_template": "share?title={title}&text={text}&url={url}",
                "action": "share",
                "method": "GET",
                "enctype": "application/x-www-form-urlencoded",
                "params": {"title": "title", "text": "text", "url": "url"},
            },
            "shortcuts": [
                {"name": "Compose new post", "url": "/publish"},
                {"name": "Notifications", "url": "/notifications"},
                {"name": "Explore", "url": "/explore"},
            ],
            "prefer_related_applications": true,
            "related_applications": [
                {
                    "platform": "play",
                    "url": "https://play.google.com/store/apps/details?id=org.joinmastodon.android",
                    "id": "org.joinmastodon.android",
                },
                {
                    "platform": "itunes",
                    "url": "https://apps.apple.com/us/app/mastodon-for-iphone/id1571998974",
                    "id": "id1571998974",
                },
                {
                    "platform": "f-droid",
                    "url": "https://f-droid.org/en/packages/org.joinmastodon.android/",
                    "id": "org.joinmastodon.android",
                },
            ],
        },
    }))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn frontend_document(
    frontend: &FrontendAssets,
    runtime: &InstanceRuntimeConfig,
    path: &str,
    instance: Option<&InstanceProjection>,
    serialized_instance: Option<serde_json::Value>,
    authenticated: Option<&FrontendAuthenticatedState>,
    csrf_token: &str,
    csp_nonce: &str,
) -> Option<String> {
    let theme = frontend.entry("styles/application.scss")?;
    let inert = frontend.entry("styles/entrypoints/inert.scss")?;
    let common = frontend.entry("entrypoints/common.ts")?;
    let application = frontend.entry("entrypoints/application.ts")?;
    let logo = frontend.asset_url("images/logo.svg")?;
    let logo_symbol = frontend.asset_url("images/logo-symbol-icon.svg")?;
    let mut initial_state = frontend_initial_state(runtime, instance, authenticated)?;
    if let Some(value) = serialized_instance {
        initial_state["instance"] = value;
    }
    let initial_state = json_script(&initial_state)?;
    let props = json_script(&serde_json::json!({"locale": "en"}))?;
    let title = instance.map_or("Mastodon", |value| value.title.as_str());
    let vapid_public_key = runtime.vapid_public_key.as_deref().unwrap_or_default();

    let mut favicon_tags = String::new();
    for size in [16_u16, 32, 48] {
        let source = format!("icons/favicon-{size}x{size}.png");
        let url = frontend.asset_url(&source)?;
        let _ = write!(
            favicon_tags,
            "<link rel=\"icon\" sizes=\"{size}x{size}\" href=\"{}\" type=\"image/png\">",
            html_escape::encode_quoted_attribute(&url),
        );
    }
    let theme_url = frontend.entry_url("styles/application.scss")?;
    let inert_url = frontend.entry_url("styles/entrypoints/inert.scss")?;
    let common_url = frontend.entry_url("entrypoints/common.ts")?;
    let application_url = frontend.entry_url("entrypoints/application.ts")?;
    let theme_integrity = integrity_attribute(theme.integrity.as_deref());
    let inert_integrity = integrity_attribute(inert.integrity.as_deref());
    let common_integrity = integrity_attribute(common.integrity.as_deref());
    let application_integrity = integrity_attribute(application.integrity.as_deref());
    let escaped_title = html_escape::encode_text(title);
    let escaped_path = html_escape::encode_quoted_attribute(path);
    let escaped_csrf = html_escape::encode_quoted_attribute(csrf_token);
    let escaped_csp_nonce = html_escape::encode_quoted_attribute(csp_nonce);
    let escaped_vapid = html_escape::encode_quoted_attribute(vapid_public_key);
    let escaped_props = html_escape::encode_quoted_attribute(&props);

    Some(format!(
        "<!doctype html><html lang=\"en\" data-contrast=\"auto\" data-color-scheme=\"auto\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover\">{favicon_tags}<link rel=\"mask-icon\" href=\"{}\" color=\"#6364FF\"><link rel=\"manifest\" href=\"/manifest\"><script nonce=\"{escaped_csp_nonce}\">{FRONTEND_THEME_SELECTION}</script><meta name=\"theme-color\" content=\"#191b22\"><meta name=\"mobile-web-app-capable\" content=\"yes\"><title>{escaped_title}</title><link rel=\"stylesheet\" href=\"{}\" media=\"all\" crossorigin=\"anonymous\"{theme_integrity}><link rel=\"stylesheet\" id=\"inert-style\" href=\"{}\" media=\"all\" crossorigin=\"anonymous\"{inert_integrity}><meta name=\"csrf-token\" content=\"{escaped_csrf}\"><meta name=\"applicationServerKey\" content=\"{escaped_vapid}\"><meta name=\"initialPath\" content=\"{escaped_path}\"><script id=\"initial-state\" type=\"application/json\" nonce=\"{escaped_csp_nonce}\">{initial_state}</script><script type=\"module\" crossorigin=\"anonymous\" src=\"{}\"{common_integrity}></script><script type=\"module\" crossorigin=\"anonymous\" src=\"{}\"{application_integrity}></script></head><body class=\"app-body\"><div class=\"notranslate app-holder\" id=\"mastodon\" data-props=\"{escaped_props}\"><noscript><img src=\"{}\" alt=\"Mastodon\"><div>JavaScript is required to use Mastodon. See <a href=\"https://joinmastodon.org/apps\">the Mastodon apps</a>.</div></noscript></div></body></html>",
        html_escape::encode_quoted_attribute(&logo_symbol),
        html_escape::encode_quoted_attribute(&theme_url),
        html_escape::encode_quoted_attribute(&inert_url),
        html_escape::encode_quoted_attribute(&common_url),
        html_escape::encode_quoted_attribute(&application_url),
        html_escape::encode_quoted_attribute(&logo),
    ))
}

pub(super) fn frontend_content_security_policy(nonce: &str) -> String {
    format!(
        "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; font-src 'self' data: https:; img-src 'self' data: blob: https:; media-src 'self' data: blob: https:; manifest-src 'self'; connect-src 'self' https: wss:; script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval'; style-src 'self'; worker-src 'self' blob:; frame-src 'self' https:"
    )
}

pub(super) fn frontend_initial_state(
    runtime: &InstanceRuntimeConfig,
    instance: Option<&InstanceProjection>,
    authenticated: Option<&FrontendAuthenticatedState>,
) -> Option<serde_json::Value> {
    let title = instance.map_or("Mastodon", |value| value.title.as_str());
    let languages = runtime
        .languages
        .iter()
        .map(|language| {
            let name = if language == "en" {
                "English"
            } else {
                language.as_str()
            };
            serde_json::json!([language, name, name])
        })
        .collect::<Vec<_>>();
    let mut initial_state = serde_json::json!({
        "accounts": {},
        "compose": {"text": ""},
        "features": [],
        "languages": languages,
        "media_attachments": {"accept_content_types": []},
        "meta": {
            "access_token": "",
            "activity_api_enabled": false,
            "admin": "",
            "auto_play_gif": true,
            "display_media": "default",
            "domain": runtime.domain,
            "landing_page": "about",
            "limited_federation_mode": runtime.limited_federation,
            "locale": "en",
            "mascot": null,
            "profile_directory": false,
            "registrations_open": instance.is_some_and(|value| value.registrations_mode != "none"),
            "reduce_motion": false,
            "repository": runtime.source_url,
            "search_enabled": false,
            "single_user_mode": runtime.single_user_mode,
            "source_url": runtime.source_url,
            "status_page_url": instance.and_then(|value| value.status_page_url.clone()),
            "streaming_api_base_url": runtime.streaming_api,
            "title": title,
            "trends_enabled": false,
            "show_trends": false,
            "use_blurhash": true,
            "version": runtime.version,
            "terms_of_service_enabled": runtime.terms_of_service_url.is_some(),
            "local_live_feed_access": instance.map_or("public", |value| value.local_live_feed_access.as_str()),
            "remote_live_feed_access": instance.map_or("public", |value| value.remote_live_feed_access.as_str()),
            "local_topic_feed_access": instance.map_or("public", |value| value.local_topic_feed_access.as_str()),
            "remote_topic_feed_access": instance.map_or("public", |value| value.remote_topic_feed_access.as_str()),
        },
        "settings": {},
    });
    if let Some(authenticated) = authenticated {
        let account = serde_json::to_value(&authenticated.account).ok()?;
        let role = serde_json::to_value(&authenticated.role).ok()?;
        let account_id = authenticated.account_id.to_string();
        initial_state["accounts"][&account_id] = account;
        initial_state["compose"]["default_language"] =
            serde_json::json!(authenticated.preferences.posting_default_language);
        initial_state["compose"]["default_privacy"] =
            serde_json::json!(authenticated.preferences.posting_default_visibility);
        initial_state["compose"]["default_quote_policy"] =
            serde_json::json!(authenticated.preferences.posting_default_quote_policy);
        initial_state["compose"]["default_sensitive"] =
            serde_json::json!(authenticated.preferences.posting_default_sensitive);
        initial_state["compose"]["me"] = serde_json::json!(account_id);
        initial_state["meta"]["access_token"] = serde_json::json!(&authenticated.access_token);
        initial_state["meta"]["me"] = serde_json::json!(account_id);
        initial_state["role"] = role;
        initial_state["settings"] = authenticated.settings.clone();
    }
    Some(initial_state)
}

pub(super) fn json_script(value: &serde_json::Value) -> Option<String> {
    Some(
        serde_json::to_string(value)
            .ok()?
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('&', "\\u0026"),
    )
}

pub(super) fn integrity_attribute(integrity: Option<&str>) -> String {
    integrity.map_or_else(String::new, |value| {
        format!(
            " integrity=\"{}\"",
            html_escape::encode_quoted_attribute(value)
        )
    })
}

pub(super) fn is_frontend_path(path: &str) -> bool {
    const EXACT: &[&str] = &[
        "/",
        "/about",
        "/blocks",
        "/bookmarks",
        "/collections",
        "/conversations",
        "/deck",
        "/directory",
        "/domain_blocks",
        "/explore",
        "/favourites",
        "/follow_requests",
        "/followed_tags",
        "/getting-started",
        "/home",
        "/keyboard-shortcuts",
        "/links",
        "/lists",
        "/mutes",
        "/notifications",
        "/notifications_v2",
        "/overview",
        "/overview/about",
        "/pinned",
        "/privacy-policy",
        "/profile",
        "/public",
        "/public/local",
        "/public/remote",
        "/publish",
        "/search",
        "/start",
        "/statuses",
        "/terms-of-service",
    ];
    if EXACT.contains(&path) || path.starts_with("/@") {
        return true;
    }
    [
        "/collections/",
        "/deck/",
        "/explore/",
        "/links/",
        "/lists/",
        "/notifications/",
        "/notifications_v2/",
        "/profile/",
        "/start/",
        "/statuses/",
        "/tags/",
        "/terms-of-service/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

pub(super) const FRONTEND_THEME_SELECTION: &str = r"(function (element) {
  const {colorScheme, contrast} = element.dataset;
  const colorSchemeMediaWatcher = window.matchMedia('(prefers-color-scheme: dark)');
  const contrastMediaWatcher = window.matchMedia('(prefers-contrast: more)');
  const updateColorScheme = () => {
    const useDarkMode = colorScheme === 'auto' ? colorSchemeMediaWatcher.matches : colorScheme === 'dark';
    element.dataset.colorScheme = useDarkMode ? 'dark' : 'light';
  };
  const updateContrast = () => {
    const useHighContrast = contrast === 'high' || contrastMediaWatcher.matches;
    element.dataset.contrast = useHighContrast ? 'high' : 'default';
  };
  colorSchemeMediaWatcher.addEventListener('change', updateColorScheme);
  contrastMediaWatcher.addEventListener('change', updateContrast);
  updateColorScheme();
  updateContrast();
})(document.documentElement);";

#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct FrontendAsset {
    pub(super) file: String,
    #[serde(default)]
    pub(super) css: Vec<String>,
    #[serde(default)]
    pub(super) imports: Vec<String>,
    #[serde(default)]
    pub(super) integrity: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct FrontendAssets {
    pub(super) root: PathBuf,
    pub(super) manifest: BTreeMap<String, FrontendAsset>,
    pub(super) asset_manifest: BTreeMap<String, FrontendAsset>,
}

pub(super) struct FrontendAuthenticatedState {
    pub(super) account: RestAccount,
    pub(super) access_token: String,
    pub(super) account_id: i64,
    pub(super) preferences: RestPreferences,
    pub(super) settings: serde_json::Value,
    pub(super) role: RestRole,
}

impl FrontendAssets {
    pub(super) fn load(root: PathBuf) -> io::Result<Self> {
        let manifest = load_frontend_manifest(&root.join("packs/.vite/manifest.json"))?;
        let asset_manifest =
            load_frontend_manifest(&root.join("packs/.vite/manifest-assets.json"))?;
        if manifest
            .values()
            .chain(asset_manifest.values())
            .any(|asset| {
                safe_frontend_path(&asset.file).is_none()
                    || asset
                        .css
                        .iter()
                        .any(|path| safe_frontend_path(path).is_none())
                    || asset
                        .imports
                        .iter()
                        .any(|path| safe_frontend_path(path).is_none())
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frontend manifest contains an unsafe asset path",
            ));
        }
        Ok(Self {
            root,
            manifest,
            asset_manifest,
        })
    }

    pub(super) fn entry(&self, source: &str) -> Option<&FrontendAsset> {
        self.manifest.get(source)
    }

    pub(super) fn asset(&self, source: &str) -> Option<&FrontendAsset> {
        self.asset_manifest.get(source)
    }

    pub(super) fn file_url(file: &str) -> Option<String> {
        Some(format!("/packs/{}", safe_frontend_path(file)?))
    }

    pub(super) fn entry_url(&self, source: &str) -> Option<String> {
        self.entry(source)
            .and_then(|asset| Self::file_url(&asset.file))
    }

    pub(super) fn asset_url(&self, source: &str) -> Option<String> {
        self.asset(source)
            .and_then(|asset| Self::file_url(&asset.file))
    }

    pub(super) fn path(&self, relative: &str) -> Option<PathBuf> {
        Some(self.root.join(safe_frontend_path(relative)?))
    }
}

pub(super) fn load_frontend_manifest(path: &FsPath) -> io::Result<BTreeMap<String, FrontendAsset>> {
    let body = std::fs::read(path)?;
    serde_json::from_slice(&body).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frontend manifest {} is invalid: {error}", path.display()),
        )
    })
}

pub(super) fn safe_frontend_path(path: &str) -> Option<&str> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return None;
    }
    if FsPath::new(path).components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return None;
    }
    Some(path)
}
