//! Browser sign-in, password reset, settings pages, sessions and CSRF.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum RustodonDocumentLayout<'a> {
    Compact,
    Settings { navigation: &'a str },
}

pub(super) fn rustodon_document(
    title: &str,
    content: &str,
    layout: RustodonDocumentLayout<'_>,
) -> String {
    let title = html_escape::encode_text(title);
    let (body_class, body) = match layout {
        RustodonDocumentLayout::Compact => (
            "rustodon rustodon--compact",
            format!("<main class=\"rustodon-card\">{content}</main>"),
        ),
        RustodonDocumentLayout::Settings { navigation } => (
            "rustodon rustodon--settings",
            format!(
                "<div class=\"settings-layout\"><aside class=\"settings-sidebar\">{navigation}</aside><main class=\"settings-content\">{content}</main></div>"
            ),
        ),
    };
    let brand = "<header class=\"rustodon-brand\"><a class=\"rustodon-brand__link\" href=\"/\" aria-label=\"Rustodon home\"><span class=\"rustodon-brand__mark\" aria-hidden=\"true\">R</span><span>Rustodon</span></a></header>";
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"color-scheme\" content=\"light dark\"><title>{title}</title><link rel=\"stylesheet\" href=\"{RUSTODON_STYLESHEET_URL}\" referrerpolicy=\"no-referrer\"></head><body class=\"{body_class}\"><div class=\"rustodon-page\">{brand}{body}</div></body></html>"
    )
}

pub(super) fn html_response(status: StatusCode, body: String) -> Response<Body> {
    let mut response = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(body))
        .expect("HTML response headers are valid");
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
        .headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert("x-xss-protection", HeaderValue::from_static("0"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("same-origin"));
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(HTML_CONTENT_SECURITY_POLICY),
    );
    response
}

pub(super) const BROWSER_SESSION_COOKIE: &str = "_mastodon_session";
pub(super) const BROWSER_CSRF_COOKIE: &str = "csrf_token";
pub(super) const SECURE_BROWSER_CSRF_COOKIE: &str = "__Host-csrf_token";
pub(super) const BROWSER_SESSION_MAX_AGE: i64 = 30 * 24 * 60 * 60;

pub(super) async fn browser_sign_in_page(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let secure = state.origin.scheme() == "https";
    let (csrf_token, set_cookie) = browser_page_csrf(&headers, secure, &state.csrf_signing_key);
    let mut response = html_response(
        StatusCode::OK,
        browser_sign_in_document(
            &csrf_token,
            None,
            None,
            browser_scalar(&parameters, "return_to"),
        ),
    );
    if let Some(cookie) = set_cookie {
        append_cookie(&mut response, &cookie);
    }
    response
}

pub(super) fn browser_page_csrf(
    headers: &HeaderMap,
    secure: bool,
    signing_key: &[u8],
) -> (String, Option<String>) {
    request_browser_csrf_cookie(headers, secure)
        .filter(|token| valid_browser_csrf_token(token, signing_key))
        .map_or_else(
            || {
                let token = new_browser_csrf_token(signing_key);
                (
                    token.clone(),
                    Some(browser_cookie(
                        browser_csrf_cookie_name(secure),
                        &token,
                        BROWSER_SESSION_MAX_AGE,
                        false,
                        secure,
                    )),
                )
            },
            |token| (token.to_owned(), None),
        )
}

pub(super) fn browser_sign_in_document(
    csrf_token: &str,
    email: Option<&str>,
    error: Option<&str>,
    return_to: Option<&str>,
) -> String {
    let email = email.map_or_else(String::new, |email| {
        html_escape::encode_quoted_attribute(email).into_owned()
    });
    let error = error.map_or_else(String::new, |message| {
        format!(
            "<p class=\"alert\" role=\"alert\">{}</p>",
            html_escape::encode_text(message)
        )
    });
    let return_to = valid_browser_return_to(return_to).map_or_else(String::new, |return_to| {
        format!(
            "<input type=\"hidden\" name=\"return_to\" value=\"{}\">",
            html_escape::encode_quoted_attribute(return_to)
        )
    });
    let content = format!(
        "<h1>Log in</h1>{error}<form method=\"post\" action=\"/auth/sign_in\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\">{return_to}<label for=\"email\">Email</label><input id=\"email\" type=\"email\" name=\"user[email]\" value=\"{email}\" autocomplete=\"username\" required><label for=\"password\">Password</label><input id=\"password\" type=\"password\" name=\"user[password]\" autocomplete=\"current-password\" required><label for=\"otp_attempt\">Two-factor or recovery code</label><input id=\"otp_attempt\" type=\"text\" name=\"user[otp_attempt]\" autocomplete=\"one-time-code\"><button type=\"submit\">Log in</button></form><p><a href=\"/auth/password/new\">Forgot your password?</a></p>",
        html_escape::encode_quoted_attribute(csrf_token),
    );
    rustodon_document("Log in", &content, RustodonDocumentLayout::Compact)
}

pub(super) fn hidden_csrf(csrf_token: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"csrf_token\" value=\"{}\">",
        html_escape::encode_quoted_attribute(csrf_token)
    )
}

pub(super) async fn required_browser_session(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<BrowserSession, Response<Body>> {
    let Some(session_id) = request_cookie(headers, BROWSER_SESSION_COOKIE) else {
        return Err(browser_redirect_response("/auth/sign_in"));
    };
    let session = match state.repository.browser_session(session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => return Err(browser_redirect_response("/auth/sign_in")),
        Err(_) => return Err(internal_error()),
    };
    if let Some(writer) = state.write_repository.as_ref()
        && !writer
            .touch_browser_session(session_id)
            .await
            .is_ok_and(|touched| touched)
    {
        return Err(browser_redirect_response("/auth/sign_in"));
    }
    if let Some(writer) = state.write_repository.as_ref() {
        writer
            .track_interactive_user(session.user_id)
            .await
            .map_err(|_| internal_error())?;
    }
    Ok(session)
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum SettingsSection {
    Profile,
    Appearance,
    PostingDefaults,
    Security,
    DeleteAccount,
}

impl SettingsSection {
    const fn path(self) -> &'static str {
        match self {
            Self::Profile => "/settings/profile",
            Self::Appearance => "/settings/preferences/appearance",
            Self::PostingDefaults => "/settings/preferences/posting_defaults",
            Self::Security => "/settings/security",
            Self::DeleteAccount => "/settings/delete",
        }
    }
}

pub(super) fn browser_settings_page(
    title: &str,
    content: &str,
    csrf_token: &str,
    csrf_cookie: Option<String>,
    active_section: Option<SettingsSection>,
) -> Response<Body> {
    let heading = format!("<h1>{}</h1>{content}", html_escape::encode_text(title));
    let navigation = browser_settings_navigation(csrf_token, active_section);
    let body = rustodon_document(
        title,
        &heading,
        RustodonDocumentLayout::Settings {
            navigation: &navigation,
        },
    );
    let mut response = html_response(StatusCode::OK, body);
    if let Some(cookie) = csrf_cookie {
        append_cookie(&mut response, &cookie);
    }
    response
}

pub(super) fn browser_settings_navigation(
    csrf_token: &str,
    active_section: Option<SettingsSection>,
) -> String {
    let links = [
        (SettingsSection::Profile, "Profile"),
        (SettingsSection::Appearance, "Appearance"),
        (SettingsSection::PostingDefaults, "Posting defaults"),
        (SettingsSection::Security, "Security"),
        (SettingsSection::DeleteAccount, "Delete account"),
    ]
    .into_iter()
    .fold(String::new(), |mut navigation, (section, label)| {
        let href = section.path();
        let current = if active_section == Some(section) {
            " aria-current=\"page\""
        } else {
            ""
        };
        let _ = write!(
            navigation,
            "<li><a href=\"{href}\"{current}>{label}</a></li>"
        );
        navigation
    });
    format!(
        "<nav class=\"settings-nav\" aria-label=\"Account settings\"><ul>{links}</ul><a href=\"/\">Back to Mastodon</a><form method=\"post\" action=\"/auth/sign_out\">{}<button class=\"button button--secondary\" type=\"submit\">Log out</button></form></nav>",
        hidden_csrf(csrf_token),
    )
}

pub(super) fn browser_settings_error_response(
    state: &WebState,
    headers: &HeaderMap,
    status: StatusCode,
    message: &str,
) -> Response<Body> {
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    let content = format!(
        "<p class=\"alert\" role=\"alert\">{}</p><p><a href=\"/settings/profile\">Return to settings</a></p>",
        html_escape::encode_text(message)
    );
    let mut response =
        browser_settings_page("Settings error", &content, &csrf_token, csrf_cookie, None);
    *response.status_mut() = status;
    response
}

pub(super) async fn browser_settings_index(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(response) = required_browser_session(&state, &headers).await {
        return response;
    }
    browser_redirect_response("/settings/profile")
}

pub(super) async fn browser_profile_page(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let account = match state.repository.account(session.account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    browser_settings_page(
        "Profile",
        &browser_profile_form(&account, &csrf_token),
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::Profile),
    )
}

pub(super) fn browser_profile_form(account: &Account, csrf_token: &str) -> String {
    let display_name = html_escape::encode_quoted_attribute(&account.display_name);
    let note = html_escape::encode_text(&account.note);
    let avatar_description = html_escape::encode_quoted_attribute(&account.avatar_description);
    let header_description = html_escape::encode_quoted_attribute(&account.header_description);
    let bot = account
        .actor_type
        .as_ref()
        .is_some_and(|value| value.0 == "Service");
    let discoverable = account.discoverable.unwrap_or(false);
    let fields = account
        .fields
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    let object = value.as_object()?;
                    Some((
                        object.get("name")?.as_str()?.to_owned(),
                        object.get("value")?.as_str()?.to_owned(),
                    ))
                })
                .take(4)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut field_inputs = String::new();
    for index in 0..4 {
        let (name, value) = fields
            .get(index)
            .cloned()
            .unwrap_or_else(|| (String::new(), String::new()));
        let name = html_escape::encode_quoted_attribute(&name);
        let value = html_escape::encode_quoted_attribute(&value);
        let _ = write!(
            field_inputs,
            "<div><label for=\"field-{index}-name\">Field {number} name</label><input id=\"field-{index}-name\" name=\"fields_attributes[{index}][name]\" value=\"{name}\" maxlength=\"255\"><label for=\"field-{index}-value\">Field {number} value</label><input id=\"field-{index}-value\" name=\"fields_attributes[{index}][value]\" value=\"{value}\" maxlength=\"255\"></div>",
            number = index + 1,
        );
    }
    format!(
        "<p>Update the profile information shown to other accounts.</p><form method=\"post\" action=\"/settings/profile\" enctype=\"multipart/form-data\">{}<fieldset><legend>Profile details</legend><label for=\"display_name\">Display name</label><input id=\"display_name\" name=\"display_name\" value=\"{display_name}\" maxlength=\"40\"><label for=\"note\">Bio</label><textarea id=\"note\" name=\"note\" maxlength=\"500\">{note}</textarea>{field_inputs}</fieldset><fieldset><legend>Account options</legend><label for=\"bot\">Account type</label><select id=\"bot\" name=\"bot\"><option value=\"0\"{}>Personal</option><option value=\"1\"{}>Bot</option></select><input type=\"hidden\" name=\"locked\" value=\"0\"><label><input type=\"checkbox\" name=\"locked\" value=\"1\"{}> Require follow requests</label><input type=\"hidden\" name=\"discoverable\" value=\"0\"><label><input type=\"checkbox\" name=\"discoverable\" value=\"1\"{}> Show account in directory</label></fieldset><fieldset><legend>Profile images</legend><label for=\"avatar\">Avatar</label><input id=\"avatar\" type=\"file\" name=\"avatar\" accept=\"image/jpeg,image/png,image/gif,image/webp\"><label for=\"avatar_description\">Avatar description</label><input id=\"avatar_description\" name=\"avatar_description\" value=\"{avatar_description}\" maxlength=\"150\"><label for=\"header\">Header</label><input id=\"header\" type=\"file\" name=\"header\" accept=\"image/jpeg,image/png,image/gif,image/webp\"><label for=\"header_description\">Header description</label><input id=\"header_description\" name=\"header_description\" value=\"{header_description}\" maxlength=\"150\"></fieldset><button type=\"submit\">Save changes</button></form>",
        hidden_csrf(csrf_token),
        if bot { "" } else { " selected" },
        if bot { " selected" } else { "" },
        if account.locked { " checked" } else { "" },
        if discoverable { " checked" } else { "" },
    )
}

pub(super) async fn browser_profile_update(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    browser_settings_account_update(state, parameters, headers, "/settings/profile").await
}

pub(super) async fn browser_posting_defaults_redirect(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(response) = required_browser_session(&state, &headers).await {
        return response;
    }
    browser_redirect_response("/settings/preferences/appearance")
}

pub(super) async fn browser_settings_appearance(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(response) = required_browser_session(&state, &headers).await {
        return response;
    }
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    let content = format!(
        "<p>The pinned Mastodon web client controls appearance settings locally. Rustodon v1 keeps the server-side appearance surface English-only.</p><p>Posting defaults are persisted server-side on the <a href=\"/settings/preferences/posting_defaults\">posting defaults page</a>.</p>{}",
        hidden_csrf(&csrf_token),
    );
    browser_settings_page(
        "Appearance",
        &content,
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::Appearance),
    )
}

pub(super) async fn browser_posting_defaults_page(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Ok(Some(preferences)) = state
        .loader(Some(session.account_id))
        .preferences(session.user_id, session.account_id)
        .await
    else {
        return internal_error();
    };
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    browser_settings_page(
        "Posting defaults",
        &browser_posting_defaults_form(&state, &preferences, &csrf_token),
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::PostingDefaults),
    )
}

pub(super) fn browser_posting_defaults_form(
    state: &WebState,
    preferences: &PreferencesProjection,
    csrf_token: &str,
) -> String {
    let visibility = settings_options(
        &[
            ("public", "Public"),
            ("unlisted", "Unlisted"),
            ("private", "Followers only"),
        ],
        &preferences.posting_default_visibility,
    );
    let quote_policy = settings_options(
        &[
            ("public", "Public"),
            ("followers", "Followers"),
            ("nobody", "Nobody"),
        ],
        &preferences.posting_default_quote_policy,
    );
    let mut language_options = String::new();
    let languages = if state.instance_runtime.languages.is_empty() {
        vec!["en".to_owned()]
    } else {
        state.instance_runtime.languages.clone()
    };
    for language in languages {
        let label = if language == "en" {
            "English"
        } else {
            language.as_str()
        };
        let _ = write!(
            language_options,
            "<option value=\"{}\"{}>{}</option>",
            html_escape::encode_quoted_attribute(&language),
            if language == preferences.posting_default_language {
                " selected"
            } else {
                ""
            },
            html_escape::encode_text(label),
        );
    }
    format!(
        "<p>These values are used when creating new posts.</p><form method=\"post\" action=\"/settings/preferences/posting_defaults\">{}<fieldset><legend>Posting defaults</legend><label for=\"default_privacy\">Default visibility</label><select id=\"default_privacy\" name=\"source[privacy]\">{visibility}</select><label for=\"default_quote_policy\">Default quote policy</label><select id=\"default_quote_policy\" name=\"source[quote_policy]\">{quote_policy}</select><label for=\"default_language\">Default language</label><select id=\"default_language\" name=\"source[language]\">{language_options}</select><input type=\"hidden\" name=\"source[sensitive]\" value=\"0\"><label><input type=\"checkbox\" name=\"source[sensitive]\" value=\"1\"{}> Mark new posts as sensitive</label></fieldset><button type=\"submit\">Save changes</button></form>",
        hidden_csrf(csrf_token),
        if preferences.posting_default_sensitive {
            " checked"
        } else {
            ""
        },
    )
}

pub(super) fn settings_options(options: &[(&str, &str)], selected: &str) -> String {
    options
        .iter()
        .fold(String::new(), |mut html, (value, label)| {
            let _ = write!(
                html,
                "<option value=\"{}\"{}>{}</option>",
                html_escape::encode_quoted_attribute(value),
                if *value == selected { " selected" } else { "" },
                html_escape::encode_text(label),
            );
            html
        })
}

pub(super) async fn browser_posting_defaults_update(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    browser_settings_account_update(
        state,
        parameters,
        headers,
        "/settings/preferences/posting_defaults",
    )
    .await
}

pub(super) async fn browser_two_factor_methods_page(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let user = match state.repository.user(session.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if !user.otp_required_for_login {
        return browser_redirect_response("/settings/otp_authentication");
    }
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    browser_settings_page(
        "Two-factor authentication",
        &browser_two_factor_methods_form(&user, &csrf_token),
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::Security),
    )
}

pub(super) fn browser_two_factor_methods_form(user: &User, csrf_token: &str) -> String {
    let backup_code_count = user.otp_backup_codes.as_ref().map_or(0, Vec::len);
    let disable = if user.role_requires_2fa {
        "<p>Your assigned role requires two-factor authentication, so it cannot be disabled here.</p>"
            .to_owned()
    } else {
        browser_disable_two_factor_form(csrf_token)
    };
    format!(
        "<p>One-time password authentication is enabled. The login form accepts a six-digit TOTP code or a recovery code.</p><p>{backup_code_count} recovery codes remain.</p><section aria-labelledby=\"recovery-codes\"><h2 id=\"recovery-codes\">Recovery codes</h2><p>Generating new recovery codes invalidates the existing set.</p><form method=\"post\" action=\"/settings/two_factor_authentication/recovery_codes\">{}<label for=\"recovery_codes_current_password\">Current password</label><input id=\"recovery_codes_current_password\" type=\"password\" name=\"current_password\" autocomplete=\"current-password\" required><button type=\"submit\">Regenerate recovery codes</button></form></section>{disable}",
        hidden_csrf(csrf_token),
        disable = disable,
    )
}

pub(super) fn browser_disable_two_factor_form(csrf_token: &str) -> String {
    format!(
        "<section class=\"danger-zone\" aria-labelledby=\"disable-two-factor\"><h2 id=\"disable-two-factor\">Disable two-factor authentication</h2><form method=\"post\" action=\"/settings/two_factor_authentication_methods/disable\">{}<label for=\"disable_current_password\">Current password</label><input id=\"disable_current_password\" type=\"password\" name=\"current_password\" autocomplete=\"current-password\" required><button class=\"button button--danger\" type=\"submit\">Disable two-factor authentication</button></form></section>",
        hidden_csrf(csrf_token),
    )
}

// Reserve a shared budget before any sensitive-settings password verification. Count
// successes too: changing route, session, or worker must not reset a guessing budget.
pub(super) async fn check_browser_reauthentication(
    state: &WebState,
    headers: &HeaderMap,
    client_ip: IpAddr,
    user_id: i64,
) -> Result<(), Response<Body>> {
    match state
        .browser_reauthentication_limiter
        .check_shared(state.shared_rate_limiter.as_ref(), client_ip, user_id)
        .await
    {
        Ok(()) => Ok(()),
        Err(limited) => {
            let mut response = browser_settings_error_response(
                state,
                headers,
                StatusCode::TOO_MANY_REQUESTS,
                "Too many password challenges. Please try again later.",
            );
            add_rate_limit_headers(&mut response, limited);
            Err(response)
        }
    }
}

pub(super) async fn browser_two_factor_disable(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The two-factor form could not be verified. Please try again.",
        );
    }
    let Some(current_password) = browser_scalar(&parameters, "current_password") else {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Enter your current password.",
        );
    };
    if let Err(response) =
        check_browser_reauthentication(&state, &headers, metadata.client_ip, session.user_id).await
    {
        return response;
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .disable_two_factor_authentication(session.user_id, current_password)
        .await
    {
        Ok(()) => browser_redirect_response("/settings/otp_authentication"),
        Err(WriteError::Unauthorized) => browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The current password is incorrect.",
        ),
        Err(WriteError::NotFound) => record_not_found(),
        Err(WriteError::InvalidInput(_) | WriteError::Validation(_)) => {
            browser_settings_error_response(
                &state,
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "Two-factor authentication could not be disabled.",
            )
        }
        Err(_) => internal_error(),
    }
}

pub(super) async fn browser_otp_authentication_page(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let user = match state.repository.user(session.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if user.otp_required_for_login {
        return browser_redirect_response("/settings/two_factor_authentication_methods");
    }
    browser_otp_setup_page_response(&state, &headers, StatusCode::OK, None)
}

pub(super) async fn browser_otp_authentication_start(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_otp_setup_page_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("The two-factor setup form could not be verified. Please try again."),
        );
    }
    let Some(current_password) = browser_scalar(&parameters, "current_password") else {
        return browser_otp_setup_page_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("Enter your current password."),
        );
    };
    let user = match state.repository.user(session.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if user.otp_required_for_login {
        return browser_redirect_response("/settings/two_factor_authentication_methods");
    }
    if let Err(response) =
        check_browser_reauthentication(&state, &headers, metadata.client_ip, session.user_id).await
    {
        return response;
    }
    if !verify_password(current_password, user.encrypted_password.as_str()) {
        return browser_otp_setup_page_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("The current password is incorrect."),
        );
    }
    let secret = random_totp_secret();
    browser_otp_confirmation_page_response(
        &state,
        &headers,
        StatusCode::OK,
        &user.email,
        &secret,
        None,
    )
}

pub(super) async fn browser_otp_confirmation_redirect(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(response) = required_browser_session(&state, &headers).await {
        return response;
    }
    browser_redirect_response("/settings/otp_authentication")
}

pub(super) async fn browser_otp_confirmation(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let user = match state.repository.user(session.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let Some(secret) = browser_scalar(&parameters, "otp_secret") else {
        return browser_redirect_response("/settings/otp_authentication");
    };
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_otp_confirmation_page_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            &user.email,
            secret,
            Some("The two-factor confirmation form could not be verified. Please try again."),
        );
    }
    let Some(current_password) = browser_scalar(&parameters, "current_password") else {
        return browser_otp_confirmation_page_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            &user.email,
            secret,
            Some("Enter your current password."),
        );
    };
    if let Err(response) =
        check_browser_reauthentication(&state, &headers, metadata.client_ip, session.user_id).await
    {
        return response;
    }
    if !verify_password(current_password, user.encrypted_password.as_str()) {
        return browser_otp_confirmation_page_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            &user.email,
            secret,
            Some("The current password is incorrect."),
        );
    }
    let Some(attempt) = browser_scalar(&parameters, "otp_attempt") else {
        return browser_otp_confirmation_page_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            &user.email,
            secret,
            Some("Enter the six-digit code from your authenticator."),
        );
    };
    if !matches!(
        verify_two_factor(Some(secret), &[], attempt, Utc::now().timestamp(), None),
        TwoFactorVerification::Totp(_)
    ) {
        return browser_otp_confirmation_page_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            &user.email,
            secret,
            Some("The authentication code is incorrect."),
        );
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .enable_two_factor_authentication(session.user_id, secret)
        .await
    {
        Ok(backup_codes) => browser_two_factor_recovery_codes_page(&state, &headers, &backup_codes),
        Err(WriteError::NotFound) => record_not_found(),
        Err(WriteError::InvalidInput(_) | WriteError::Validation(_)) => {
            browser_otp_confirmation_page_response(
                &state,
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                &user.email,
                secret,
                Some("Two-factor authentication could not be enabled."),
            )
        }
        Err(_) => internal_error(),
    }
}

pub(super) async fn browser_two_factor_recovery_codes(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The recovery-code form could not be verified. Please try again.",
        );
    }
    let Some(current_password) = browser_scalar(&parameters, "current_password") else {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Enter your current password.",
        );
    };
    if let Err(response) =
        check_browser_reauthentication(&state, &headers, metadata.client_ip, session.user_id).await
    {
        return response;
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .regenerate_two_factor_backup_codes(session.user_id, current_password)
        .await
    {
        Ok(backup_codes) => browser_two_factor_recovery_codes_page(&state, &headers, &backup_codes),
        Err(WriteError::Unauthorized) => browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The current password is incorrect.",
        ),
        Err(WriteError::NotFound) => record_not_found(),
        Err(WriteError::InvalidInput(_) | WriteError::Validation(_)) => {
            browser_settings_error_response(
                &state,
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "Recovery codes could not be regenerated.",
            )
        }
        Err(_) => internal_error(),
    }
}

pub(super) fn browser_otp_setup_page_response(
    state: &WebState,
    headers: &HeaderMap,
    status: StatusCode,
    error: Option<&str>,
) -> Response<Body> {
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    let error = error.map_or_else(String::new, |message| {
        format!(
            "<p class=\"alert\" role=\"alert\">{}</p>",
            html_escape::encode_text(message)
        )
    });
    let content = format!(
        "{error}<p>Set up two-factor authentication with an authenticator application. You will confirm a generated secret before it is stored.</p><form method=\"post\" action=\"/settings/otp_authentication\">{}<label for=\"setup_current_password\">Current password</label><input id=\"setup_current_password\" type=\"password\" name=\"current_password\" autocomplete=\"current-password\" required><button type=\"submit\">Set up two-factor authentication</button></form>",
        hidden_csrf(&csrf_token),
    );
    let mut response = browser_settings_page(
        "Set up two-factor authentication",
        &content,
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::Security),
    );
    *response.status_mut() = status;
    response
}

pub(super) fn browser_otp_confirmation_page_response(
    state: &WebState,
    headers: &HeaderMap,
    status: StatusCode,
    email: &str,
    secret: &str,
    error: Option<&str>,
) -> Response<Body> {
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    let error = error.map_or_else(String::new, |message| {
        format!(
            "<p class=\"alert\" role=\"alert\">{}</p>",
            html_escape::encode_text(message)
        )
    });
    let uri = browser_totp_provisioning_uri(secret, email, &state.local_domain);
    let content = format!(
        "{error}<p>Scan this provisioning URI in your authenticator application, or enter the secret manually.</p><p><code>{}</code></p><p>Manual secret: <samp>{}</samp></p><form method=\"post\" action=\"/settings/two_factor_authentication/confirmation\">{}<input type=\"hidden\" name=\"otp_secret\" value=\"{}\"><label for=\"confirmation_current_password\">Current password</label><input id=\"confirmation_current_password\" type=\"password\" name=\"current_password\" autocomplete=\"current-password\" required><label for=\"otp_attempt\">Authentication code</label><input id=\"otp_attempt\" type=\"text\" name=\"otp_attempt\" inputmode=\"numeric\" autocomplete=\"one-time-code\" pattern=\"[0-9]{{6}}\" required><button type=\"submit\">Enable two-factor authentication</button></form>",
        html_escape::encode_text(&uri),
        html_escape::encode_text(&spaced_totp_secret(secret)),
        hidden_csrf(&csrf_token),
        html_escape::encode_quoted_attribute(secret),
    );
    let mut response = browser_settings_page(
        "Confirm two-factor authentication",
        &content,
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::Security),
    );
    *response.status_mut() = status;
    response
}

pub(super) fn browser_two_factor_recovery_codes_page(
    state: &WebState,
    headers: &HeaderMap,
    backup_codes: &[String],
) -> Response<Body> {
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    let codes = backup_codes.iter().fold(String::new(), |mut codes, code| {
        let _ = write!(
            codes,
            "<li><samp>{}</samp></li>",
            html_escape::encode_text(code),
        );
        codes
    });
    let content = format!(
        "<p><strong>Recovery codes</strong> can be used once each when your authenticator is unavailable. Store them somewhere safe.</p><ol class=\"recovery-codes\">{codes}</ol><p>These codes will not be shown again.</p>{}",
        hidden_csrf(&csrf_token),
    );
    browser_settings_page(
        "Recovery codes",
        &content,
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::Security),
    )
}

pub(super) fn browser_totp_provisioning_uri(secret: &str, email: &str, issuer: &str) -> String {
    let label_source = format!("{issuer}:{email}");
    let label = utf8_percent_encode(&label_source, NON_ALPHANUMERIC);
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("secret", secret);
    query.append_pair("issuer", issuer);
    query.append_pair("algorithm", "SHA1");
    query.append_pair("digits", "6");
    query.append_pair("period", "30");
    format!("otpauth://totp/{label}?{}", query.finish())
}

pub(super) fn spaced_totp_secret(secret: &str) -> String {
    secret
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) async fn browser_security_page(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let user = match state.repository.user(session.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let (csrf_token, csrf_cookie) = browser_page_csrf(
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    );
    let two_factor = if user.otp_required_for_login {
        "enabled"
    } else {
        "not enabled"
    };
    let webauthn = if user.has_webauthn_credentials {
        "configured"
    } else {
        "not configured"
    };
    let two_factor_action = if user.otp_required_for_login {
        "<a href=\"/settings/two_factor_authentication_methods\">Manage two-factor authentication</a>"
    } else {
        "<a href=\"/settings/otp_authentication\">Set up two-factor authentication</a>"
    };
    let content = format!(
        "<p>Signed in as <strong>{}</strong>.</p><section aria-labelledby=\"two-factor\"><h2 id=\"two-factor\">Two-factor authentication</h2><p>One-time password authentication is {two_factor}; security keys are {webauthn}. The login form accepts a TOTP or backup code.</p><p>{two_factor_action}</p></section><section aria-labelledby=\"password\"><h2 id=\"password\">Change password</h2><form method=\"post\" action=\"/settings/security\">{}<label for=\"current_password\">Current password</label><input id=\"current_password\" type=\"password\" name=\"current_password\" autocomplete=\"current-password\" required><label for=\"password\">New password</label><input id=\"password\" type=\"password\" name=\"password\" autocomplete=\"new-password\" required><label for=\"password_confirmation\">Confirm new password</label><input id=\"password_confirmation\" type=\"password\" name=\"password_confirmation\" autocomplete=\"new-password\" required><button type=\"submit\">Change password</button></form></section>",
        html_escape::encode_text(&user.email),
        hidden_csrf(&csrf_token),
    );
    browser_settings_page(
        "Security",
        &content,
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::Security),
    )
}

pub(super) async fn browser_security_update(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The security form could not be verified. Please try again.",
        );
    }
    let Some(current_password) = browser_scalar(&parameters, "current_password") else {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Enter your current password.",
        );
    };
    let Some(password) = browser_scalar(&parameters, "password") else {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Enter a new password.",
        );
    };
    if browser_scalar(&parameters, "password_confirmation") != Some(password) {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The new password confirmation does not match.",
        );
    }
    if let Err(response) =
        check_browser_reauthentication(&state, &headers, metadata.client_ip, session.user_id).await
    {
        return response;
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let authentication = match writer
        .verify_current_password(session.user_id, current_password)
        .await
    {
        Ok(authentication) => authentication,
        Err(WriteError::Unauthorized) => {
            return browser_settings_error_response(
                &state,
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "The current password is incorrect.",
            );
        }
        Err(_) => return internal_error(),
    };
    match writer.change_user_password(&authentication, password).await {
        Ok(()) => browser_redirect_response("/auth/sign_in"),
        Err(WriteError::Unauthorized | WriteError::InvalidInput(_) | WriteError::Validation(_)) => {
            browser_settings_error_response(
                &state,
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "The new password could not be saved. Please authenticate again.",
            )
        }
        Err(_) => internal_error(),
    }
}

pub(super) async fn browser_delete_page(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let user = match state.repository.user(session.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    browser_delete_page_response(
        &state,
        state.origin.scheme() == "https",
        &headers,
        StatusCode::OK,
        &user,
        None,
    )
}

pub(super) async fn browser_delete(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let account = match state.repository.account(session.account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let user = match state.repository.user(session.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_delete_page_response(
            &state,
            state.origin.scheme() == "https",
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            &user,
            Some("The deletion form could not be verified. Please try again."),
        );
    }
    if let Err(response) =
        check_browser_reauthentication(&state, &headers, metadata.client_ip, session.user_id).await
    {
        return response;
    }
    let challenge_passed = if user.encrypted_password.is_present() {
        browser_scalar(&parameters, "password")
            .is_some_and(|password| verify_password(password, user.encrypted_password.as_str()))
    } else {
        browser_scalar(&parameters, "username").is_some_and(|username| username == account.username)
    };
    if !challenge_passed {
        return browser_delete_page_response(
            &state,
            state.origin.scheme() == "https",
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            &user,
            Some("The password or username confirmation is incorrect."),
        );
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let actor_uri = activitypub::actor_url(&state.origin, &account);
    match writer
        .request_account_deletion(session.account_id, &actor_uri)
        .await
    {
        Ok(()) => {}
        Err(WriteError::InvalidInput(_) | WriteError::Validation(_)) => {
            return browser_delete_page_response(
                &state,
                state.origin.scheme() == "https",
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                &user,
                Some("The account could not be scheduled for deletion."),
            );
        }
        Err(WriteError::NotFound) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    let Some(session_id) = request_cookie(&headers, BROWSER_SESSION_COOKIE) else {
        return internal_error();
    };
    if writer.delete_browser_session(session_id).await.is_err() {
        return internal_error();
    }
    browser_signed_out_redirect_response(state.origin.scheme() == "https")
}

pub(super) fn browser_delete_page_response(
    state: &WebState,
    secure: bool,
    headers: &HeaderMap,
    status: StatusCode,
    user: &User,
    error: Option<&str>,
) -> Response<Body> {
    let (csrf_token, csrf_cookie) = browser_page_csrf(headers, secure, &state.csrf_signing_key);
    let error = error.map_or_else(String::new, |message| {
        format!(
            "<p class=\"alert\" role=\"alert\">{}</p>",
            html_escape::encode_text(message)
        )
    });
    let content = format!(
        "{error}<p>Deleting your account is irreversible. Your account will become unavailable immediately and the deletion request will be queued for processing.</p>{}",
        browser_delete_form(user.encrypted_password.is_present(), &csrf_token),
    );
    let mut response = browser_settings_page(
        "Delete account",
        &content,
        &csrf_token,
        csrf_cookie,
        Some(SettingsSection::DeleteAccount),
    );

    *response.status_mut() = status;
    response
}

pub(super) fn browser_delete_form(has_password: bool, csrf_token: &str) -> String {
    let challenge = if has_password {
        "<label for=\"password\">Password</label><input id=\"password\" type=\"password\" name=\"password\" autocomplete=\"current-password\" required>"
            .to_owned()
    } else {
        "<label for=\"username\">Username</label><input id=\"username\" type=\"text\" name=\"username\" autocomplete=\"off\" required>"
            .to_owned()
    };
    format!(
        "<form class=\"danger-zone\" method=\"post\" action=\"/settings/delete\">{}{challenge}<button class=\"button button--danger\" type=\"submit\">Delete account</button></form>",
        hidden_csrf(csrf_token),
    )
}

pub(super) async fn browser_settings_account_update(
    state: WebState,
    parameters: RackParameters,
    headers: HeaderMap,
    redirect: &'static str,
) -> Response<Body> {
    let session = match required_browser_session(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_settings_error_response(
            &state,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The settings form could not be verified. Please try again.",
        );
    }
    let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", session.access_token.as_str()))
    else {
        return internal_error();
    };
    let error_state = state.clone();
    let error_headers = headers.clone();
    let mut api_headers = headers;
    api_headers.insert(AUTHORIZATION, value);
    let response = update_credentials(State(state), Extension(parameters), api_headers).await;
    if response.status().is_success() {
        browser_redirect_response(redirect)
    } else {
        browser_settings_error_response(
            &error_state,
            &error_headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The settings could not be saved.",
        )
    }
}

pub(super) async fn browser_password_reset_page(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    browser_password_reset_page_response(
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
        &headers,
        StatusCode::OK,
        None,
        None,
    )
}

pub(super) async fn browser_password_reset_edit(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(token) = browser_scalar(&parameters, "reset_password_token")
        .filter(|token| !token.trim().is_empty())
    else {
        return browser_redirect_response("/auth/password/new");
    };
    browser_password_reset_page_response(
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
        &headers,
        StatusCode::OK,
        Some(token),
        None,
    )
}

pub(super) fn browser_password_reset_page_response(
    secure: bool,
    signing_key: &[u8],
    headers: &HeaderMap,
    status: StatusCode,
    reset_token: Option<&str>,
    error: Option<&str>,
) -> Response<Body> {
    let (csrf_token, set_cookie) = browser_page_csrf(headers, secure, signing_key);
    let mut response = html_response(
        status,
        browser_password_reset_document(&csrf_token, reset_token, error),
    );
    if let Some(cookie) = set_cookie {
        append_cookie(&mut response, &cookie);
    }
    response
}

pub(super) fn browser_password_reset_document(
    csrf_token: &str,
    reset_token: Option<&str>,
    error: Option<&str>,
) -> String {
    let error = error.map_or_else(String::new, |message| {
        format!(
            "<p class=\"alert\" role=\"alert\">{}</p>",
            html_escape::encode_text(message)
        )
    });
    let csrf_token = html_escape::encode_quoted_attribute(csrf_token);
    let (title, content) = match reset_token {
        Some(reset_token) => (
            "Choose a new password",
            format!(
                "<h1>Choose a new password</h1>{error}<form method=\"post\" action=\"/auth/password\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf_token}\"><input type=\"hidden\" name=\"reset_password_token\" value=\"{}\"><label>New password<input type=\"password\" name=\"user[password]\" autocomplete=\"new-password\" required></label><label>Confirm password<input type=\"password\" name=\"user[password_confirmation]\" autocomplete=\"new-password\" required></label><button type=\"submit\">Change password</button></form>",
                html_escape::encode_quoted_attribute(reset_token),
            ),
        ),
        None => (
            "Reset password",
            format!(
                "<h1>Reset password</h1>{error}<p>If the account exists, instructions will be sent.</p><form method=\"post\" action=\"/auth/password\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf_token}\"><label>Email<input type=\"email\" name=\"user[email]\" autocomplete=\"email\" required></label><button type=\"submit\">Send reset instructions</button></form>"
            ),
        ),
    };
    rustodon_document(title, &content, RustodonDocumentLayout::Compact)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn browser_sign_in_error_response(
    secure: bool,
    signing_key: &[u8],
    headers: &HeaderMap,
    status: StatusCode,
    code: &str,
    message: &str,
    email: Option<&str>,
    return_to: Option<&str>,
) -> Response<Body> {
    if !accepts_html(headers) {
        return browser_auth_error_response(status, code);
    }
    let (csrf_token, set_cookie) = browser_page_csrf(headers, secure, signing_key);
    let mut response = html_response(
        status,
        browser_sign_in_document(&csrf_token, email, Some(message), return_to),
    );
    if let Some(cookie) = set_cookie {
        append_cookie(&mut response, &cookie);
    }
    response
}

pub(super) fn browser_password_reset_error_response(
    secure: bool,
    signing_key: &[u8],
    headers: &HeaderMap,
    status: StatusCode,
    code: &str,
    message: &str,
    reset_token: Option<&str>,
) -> Response<Body> {
    if !accepts_html(headers) {
        return browser_auth_error_response(status, code);
    }
    browser_password_reset_page_response(
        secure,
        signing_key,
        headers,
        status,
        reset_token,
        Some(message),
    )
}

pub(super) fn browser_confirmation_error_response(
    headers: &HeaderMap,
    status: StatusCode,
    code: &str,
    message: &str,
) -> Response<Body> {
    if !accepts_html(headers) {
        return browser_auth_error_response(status, code);
    }
    let content = format!(
        "<h1>Confirmation error</h1><p class=\"alert\" role=\"alert\">{}</p><p><a href=\"/auth/sign_in\">Return to log in</a></p>",
        html_escape::encode_text(message),
    );
    html_response(
        status,
        rustodon_document(
            "Confirmation error",
            &content,
            RustodonDocumentLayout::Compact,
        ),
    )
}

pub(super) fn browser_sign_in_rate_limited_response(
    secure: bool,
    signing_key: &[u8],
    headers: &HeaderMap,
    limited: RateLimitExceeded,
    return_to: Option<&str>,
) -> Response<Body> {
    let mut response = browser_sign_in_error_response(
        secure,
        signing_key,
        headers,
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "Too many login attempts. Please try again later.",
        None,
        return_to,
    );
    add_rate_limit_headers(&mut response, limited);
    response
}

pub(super) fn browser_password_reset_rate_limited_response(
    secure: bool,
    signing_key: &[u8],
    headers: &HeaderMap,
    limited: RateLimitExceeded,
) -> Response<Body> {
    let mut response = browser_password_reset_error_response(
        secure,
        signing_key,
        headers,
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "Too many password reset requests. Please try again later.",
        None,
    );
    add_rate_limit_headers(&mut response, limited);
    response
}

pub(super) async fn browser_password_reset_request(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    Extension(metadata): Extension<RequestMetadata>,
    headers: HeaderMap,
) -> Response<Body> {
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_password_reset_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_csrf_token",
            "The reset form could not be verified. Please try again.",
            None,
        );
    }
    let Some(email) = browser_scalar(&parameters, "email").filter(|email| !email.trim().is_empty())
    else {
        return browser_password_reset_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "Enter your email address.",
            None,
        );
    };
    if matches!(state.mail_config.as_ref(), Some(config) if !config.is_enabled()) {
        return browser_password_reset_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::SERVICE_UNAVAILABLE,
            "mail_unavailable",
            "Password reset email is currently unavailable.",
            None,
        );
    }
    if let Err(limited) = state
        .password_reset_limiter
        .check_shared(
            state.shared_rate_limiter.as_ref(),
            metadata.client_ip,
            email,
        )
        .await
    {
        return browser_password_reset_rate_limited_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            limited,
        );
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let result = match state.mail_config.as_ref() {
        Some(mail_config) if mail_config.is_enabled() => {
            writer
                .create_password_reset_token_with_job(
                    email,
                    Some(mail_config.token_digest_secret()),
                    |token| {
                        mail_config
                            .password_reset_job(email, token)
                            .map_err(|_| WriteError::Validation("reset mail job is invalid"))
                    },
                )
                .await
        }
        Some(_) => unreachable!("disabled SMTP was rejected before token creation"),
        None => writer.create_password_reset_token(email).await,
    };
    if result.is_err() {
        return internal_error();
    }
    browser_redirect_response("/auth/password/new")
}

pub(super) async fn browser_confirmation(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(token) =
        browser_scalar(&parameters, "confirmation_token").filter(|token| !token.trim().is_empty())
    else {
        return browser_confirmation_error_response(
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_confirmation_token",
            "The confirmation link is invalid or has expired.",
        );
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let result = match state.mail_config.as_ref() {
        Some(mail_config) => {
            writer
                .confirm_user_with_token_and_secret(token, mail_config.token_digest_secret())
                .await
        }
        None => writer.confirm_user_with_token(token).await,
    };
    match result {
        Ok(true) => browser_redirect_response("/auth/sign_in"),
        Ok(false) | Err(WriteError::InvalidInput(_) | WriteError::Validation(_)) => {
            browser_confirmation_error_response(
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_confirmation_token",
                "The confirmation link is invalid or has expired.",
            )
        }
        Err(_) => internal_error(),
    }
}

pub(super) async fn browser_password_reset_update(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_password_reset_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_csrf_token",
            "The reset form could not be verified. Please try again.",
            browser_scalar(&parameters, "reset_password_token"),
        );
    }
    let Some(token) = browser_scalar(&parameters, "reset_password_token")
        .filter(|token| !token.trim().is_empty())
    else {
        return browser_password_reset_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_reset_token",
            "The reset link is invalid or has expired.",
            None,
        );
    };
    let Some(password) = browser_scalar(&parameters, "password") else {
        return browser_password_reset_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_password",
            "Enter a new password.",
            Some(token),
        );
    };
    if browser_scalar(&parameters, "password_confirmation") != Some(password) {
        return browser_password_reset_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "password_confirmation_mismatch",
            "The password confirmation does not match.",
            Some(token),
        );
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let result = match state.mail_config.as_ref() {
        Some(mail_config) => {
            writer
                .reset_password_with_token_and_secret(
                    token,
                    password,
                    mail_config.token_digest_secret(),
                )
                .await
        }
        None => writer.reset_password_with_token(token, password).await,
    };
    match result {
        Ok(true) => browser_redirect_response("/auth/sign_in"),
        Ok(false) | Err(WriteError::InvalidInput(_) | WriteError::Validation(_)) => {
            browser_password_reset_error_response(
                state.origin.scheme() == "https",
                &state.csrf_signing_key,
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_reset_token",
                "The reset link is invalid or has expired.",
                Some(token),
            )
        }
        Err(_) => internal_error(),
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn browser_sign_in(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    Extension(metadata): Extension<RequestMetadata>,
    headers: HeaderMap,
) -> Response<Body> {
    let return_to = valid_browser_return_to(browser_scalar(&parameters, "return_to"));
    if !browser_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_sign_in_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_csrf_token",
            "The login form could not be verified. Please try again.",
            None,
            return_to,
        );
    }
    let Some(email) = browser_scalar(&parameters, "email").filter(|value| !value.trim().is_empty())
    else {
        return browser_sign_in_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Enter your email address.",
            None,
            return_to,
        );
    };
    let Some(password) = browser_scalar(&parameters, "password") else {
        return browser_sign_in_error_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Enter your password.",
            Some(email),
            return_to,
        );
    };
    if let Err(limited) = state
        .browser_login_limiter
        .check_shared(
            state.shared_rate_limiter.as_ref(),
            metadata.client_ip,
            email,
        )
        .await
    {
        return browser_sign_in_rate_limited_response(
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
            &headers,
            limited,
            return_to,
        );
    }
    let otp_attempt = browser_scalar(&parameters, "otp_attempt");
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let ip = IpNetwork::from(metadata.client_ip);
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let authentication = match writer
        .authenticate_browser_user(
            email,
            password,
            otp_attempt,
            Utc::now().timestamp(),
            ip,
            user_agent,
        )
        .await
    {
        Ok(authentication) => authentication,
        Err(error) => {
            return browser_authentication_error_response(
                state.origin.scheme() == "https",
                &state.csrf_signing_key,
                &headers,
                email,
                &error,
                return_to,
            );
        }
    };
    let session_id = match writer
        .create_browser_session(&authentication, ip, user_agent)
        .await
    {
        Ok(session_id) => session_id,
        Err(error) => {
            return browser_session_creation_error_response(
                state.origin.scheme() == "https",
                &state.csrf_signing_key,
                &headers,
                email,
                &error,
                return_to,
            );
        }
    };
    let csrf_token = new_browser_csrf_token(&state.csrf_signing_key);
    let mut response = browser_redirect_response(return_to.unwrap_or("/"));
    let secure = state.origin.scheme() == "https";
    append_cookie(
        &mut response,
        &browser_cookie(
            BROWSER_SESSION_COOKIE,
            &session_id,
            BROWSER_SESSION_MAX_AGE,
            true,
            secure,
        ),
    );
    append_cookie(
        &mut response,
        &browser_cookie(
            browser_csrf_cookie_name(secure),
            &csrf_token,
            BROWSER_SESSION_MAX_AGE,
            false,
            secure,
        ),
    );
    response
}

pub(super) async fn browser_session(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(session_id) = request_cookie(&headers, BROWSER_SESSION_COOKIE) else {
        return browser_auth_error_response(StatusCode::UNAUTHORIZED, "unauthenticated");
    };
    let session = match state.repository.browser_session(session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return browser_auth_error_response(StatusCode::UNAUTHORIZED, "unauthenticated");
        }
        Err(_) => return internal_error(),
    };
    if let Some(writer) = state.write_repository.as_ref()
        && !writer
            .touch_browser_session(session_id)
            .await
            .is_ok_and(|touched| touched)
    {
        return browser_auth_error_response(StatusCode::UNAUTHORIZED, "unauthenticated");
    }
    if let Some(writer) = state.write_repository.as_ref()
        && writer
            .track_interactive_user(session.user_id)
            .await
            .is_err()
    {
        return internal_error();
    }
    let value = serde_json::json!({
        "authenticated": true,
        "user_id": session.user_id.to_string(),
        "account_id": session.account_id.to_string(),
        "last_seen_at": session.updated_at.and_utc().to_rfc3339(),
    });
    browser_json_response(StatusCode::OK, &value)
}

pub(super) async fn browser_sign_out(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(session_id) = request_cookie(&headers, BROWSER_SESSION_COOKIE) else {
        return if accepts_html(&headers) {
            browser_signed_out_redirect_response(state.origin.scheme() == "https")
        } else {
            browser_signed_out_response(state.origin.scheme() == "https")
        };
    };
    if !browser_sign_out_csrf_is_valid(
        &parameters,
        &headers,
        state.origin.scheme() == "https",
        &state.csrf_signing_key,
    ) {
        return browser_sign_out_error_response(&state, &headers);
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if writer.delete_browser_session(session_id).await.is_err() {
        return internal_error();
    }
    if accepts_html(&headers) {
        browser_signed_out_redirect_response(state.origin.scheme() == "https")
    } else {
        browser_signed_out_response(state.origin.scheme() == "https")
    }
}

pub(super) fn browser_sign_out_csrf_is_valid(
    parameters: &RackParameters,
    headers: &HeaderMap,
    secure: bool,
    signing_key: &[u8],
) -> bool {
    browser_csrf_is_valid(parameters, headers, secure, signing_key)
        || request_browser_csrf_cookie(headers, secure).is_some_and(|csrf_cookie| {
            valid_browser_csrf_token(csrf_cookie, signing_key)
                && headers
                    .get("x-csrf-token")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|csrf_header| {
                        constant_time_equal(csrf_cookie.as_bytes(), csrf_header.as_bytes())
                    })
        })
}

pub(super) fn browser_sign_out_error_response(
    state: &WebState,
    headers: &HeaderMap,
) -> Response<Body> {
    if accepts_html(headers) {
        browser_settings_error_response(
            state,
            headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The sign-out form could not be verified. Please try again.",
        )
    } else {
        browser_auth_error_response(StatusCode::FORBIDDEN, "invalid_csrf_token")
    }
}

pub(super) fn browser_scalar<'a>(parameters: &'a RackParameters, name: &str) -> Option<&'a str> {
    let value = parameters
        .get(name)
        .or_else(|| match parameters.get("user") {
            Some(RackValue::Object(values)) => values.get(name),
            _ => None,
        })?;
    match value {
        RackValue::Scalar(value) => Some(value),
        _ => None,
    }
}

pub(super) fn browser_csrf_is_valid(
    parameters: &RackParameters,
    headers: &HeaderMap,
    secure: bool,
    signing_key: &[u8],
) -> bool {
    let Some(cookie) = request_browser_csrf_cookie(headers, secure) else {
        return false;
    };
    let Some(attempt) = browser_scalar(parameters, BROWSER_CSRF_COOKIE) else {
        return false;
    };
    valid_browser_csrf_token(cookie, signing_key)
        && constant_time_equal(cookie.as_bytes(), attempt.as_bytes())
}

/// A session denied after authentication (a password recovery raced the login)
/// is an ordinary credential failure; it must not reveal the recovery.
pub(super) fn browser_session_creation_error_response(
    secure: bool,
    signing_key: &[u8],
    headers: &HeaderMap,
    email: &str,
    error: &WriteError,
    return_to: Option<&str>,
) -> Response<Body> {
    if matches!(error, WriteError::Unauthorized) {
        browser_authentication_error_response(
            secure,
            signing_key,
            headers,
            email,
            &BrowserAuthenticationError::InvalidCredentials,
            return_to,
        )
    } else {
        internal_error()
    }
}

pub(super) fn browser_authentication_error_response(
    secure: bool,
    signing_key: &[u8],
    headers: &HeaderMap,
    email: &str,
    error: &BrowserAuthenticationError,
    return_to: Option<&str>,
) -> Response<Body> {
    match error {
        BrowserAuthenticationError::InvalidCredentials => browser_sign_in_error_response(
            secure,
            signing_key,
            headers,
            if accepts_html(headers) {
                StatusCode::OK
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            },
            "invalid_credentials",
            "Invalid email or password.",
            Some(email),
            return_to,
        ),
        BrowserAuthenticationError::Unconfirmed
        | BrowserAuthenticationError::PendingApproval
        | BrowserAuthenticationError::Memorialized => browser_sign_in_redirect(return_to),
        BrowserAuthenticationError::TwoFactorRequired => browser_sign_in_error_response(
            secure,
            signing_key,
            headers,
            StatusCode::OK,
            "two_factor_required",
            "Enter your two-factor or recovery code.",
            Some(email),
            return_to,
        ),
        BrowserAuthenticationError::InvalidTwoFactor => browser_sign_in_error_response(
            secure,
            signing_key,
            headers,
            StatusCode::OK,
            "invalid_two_factor",
            "The two-factor or recovery code is invalid.",
            Some(email),
            return_to,
        ),
        BrowserAuthenticationError::RateLimited => browser_sign_in_error_response(
            secure,
            signing_key,
            headers,
            StatusCode::OK,
            "rate_limited",
            "Too many two-factor attempts. Please try again later.",
            Some(email),
            return_to,
        ),
        BrowserAuthenticationError::Database(_) => internal_error(),
    }
}

pub(super) fn browser_redirect_response(location: &str) -> Response<Body> {
    let mut response = Response::builder()
        .status(StatusCode::FOUND)
        .header(LOCATION, location)
        .body(Body::empty())
        .expect("browser redirect response headers are valid");
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
        .headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

pub(super) fn valid_browser_return_to(value: Option<&str>) -> Option<&str> {
    let value = value?;
    if value.is_empty()
        || !value.starts_with('/')
        || value.starts_with("//")
        || value.contains('\\')
        || value.contains(['\r', '\n'])
    {
        return None;
    }
    value.parse::<Uri>().ok().map(|_| value)
}

pub(super) fn browser_sign_in_redirect(return_to: Option<&str>) -> Response<Body> {
    let Some(return_to) = valid_browser_return_to(return_to) else {
        return browser_redirect_response("/auth/sign_in");
    };
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("return_to", return_to);
    browser_redirect_response(&format!("/auth/sign_in?{}", query.finish()))
}

pub(super) fn oauth_authorize_sign_in_redirect(parameters: &RackParameters) -> Response<Body> {
    let query = parameters.to_query();
    let return_to = if query.is_empty() {
        "/oauth/authorize".to_owned()
    } else {
        format!("/oauth/authorize?{query}")
    };
    browser_sign_in_redirect(Some(&return_to))
}

pub(super) fn browser_auth_error_response(status: StatusCode, error: &str) -> Response<Body> {
    let value = serde_json::json!({ "error": error });
    let mut response = browser_json_response(status, &value);
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"Rustodon\""),
        );
    }
    response
}

pub(super) fn browser_json_response(
    status: StatusCode,
    value: &serde_json::Value,
) -> Response<Body> {
    let mut response = json_response(
        status,
        serde_json::to_vec(&value).expect("browser authentication response is serializable"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
        .headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

pub(super) fn browser_signed_out_response(secure: bool) -> Response<Body> {
    let value = serde_json::json!({ "redirect_to": "/auth/sign_in" });
    let mut response = browser_json_response(StatusCode::OK, &value);
    append_cookie(
        &mut response,
        &browser_cookie(BROWSER_SESSION_COOKIE, "", 0, true, secure),
    );
    append_cookie(
        &mut response,
        &browser_cookie(browser_csrf_cookie_name(secure), "", 0, false, secure),
    );
    response
}

pub(super) fn browser_signed_out_redirect_response(secure: bool) -> Response<Body> {
    let mut response = browser_redirect_response("/auth/sign_in");
    append_cookie(
        &mut response,
        &browser_cookie(BROWSER_SESSION_COOKIE, "", 0, true, secure),
    );
    append_cookie(
        &mut response,
        &browser_cookie(browser_csrf_cookie_name(secure), "", 0, false, secure),
    );
    response
}

pub(super) fn request_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut result = None;
    for value in headers.get_all(COOKIE) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for cookie in value.split(';') {
            let Some((cookie_name, cookie_value)) = cookie.trim().split_once('=') else {
                continue;
            };
            if cookie_name == name {
                if result.is_some() {
                    return None;
                }
                result = Some(cookie_value);
            }
        }
    }
    result
}

pub(super) fn browser_csrf_cookie_name(secure: bool) -> &'static str {
    if secure {
        SECURE_BROWSER_CSRF_COOKIE
    } else {
        BROWSER_CSRF_COOKIE
    }
}

pub(super) fn request_browser_csrf_cookie(headers: &HeaderMap, secure: bool) -> Option<&str> {
    request_cookie(headers, browser_csrf_cookie_name(secure))
}

pub(super) fn derive_browser_csrf_signing_key(secret: &str) -> [u8; 32] {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(b"rustodon/browser-csrf/signing-key/v1");
    mac.finalize().into_bytes().into()
}

pub(super) fn new_browser_csrf_token(signing_key: &[u8]) -> String {
    let nonce = random_auth_token(32);
    let mut mac =
        Hmac::<Sha256>::new_from_slice(signing_key).expect("HMAC accepts keys of any size");
    mac.update(nonce.as_bytes());
    format!(
        "{nonce}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

pub(super) fn valid_browser_csrf_token(token: &str, signing_key: &[u8]) -> bool {
    let Some((nonce, signature)) = token.split_once('.') else {
        return false;
    };
    if nonce.is_empty() || signature.contains('.') {
        return false;
    }
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(signing_key).expect("HMAC accepts keys of any size");
    mac.update(nonce.as_bytes());
    mac.verify_slice(&signature).is_ok()
}

pub(super) fn browser_cookie(
    name: &str,
    value: &str,
    max_age: i64,
    http_only: bool,
    secure: bool,
) -> String {
    let mut cookie = format!("{name}={value}; Path=/; Max-Age={max_age}; SameSite=Lax");
    if http_only {
        cookie.push_str("; HttpOnly");
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub(super) fn append_cookie(response: &mut Response<Body>, cookie: &str) {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
}
