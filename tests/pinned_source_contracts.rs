use std::fs;
use std::path::{Path, PathBuf};

use rustodon::web::{API_ROUTE_INVENTORY, ApiMethod, ApiRouteSupport};

#[derive(Clone, Copy)]
struct FrontendRouteContract {
    label: &'static str,
    source: &'static str,
    evidence: &'static str,
    path: &'static str,
    method: ApiMethod,
    support: ApiRouteSupport,
    router_handler: &'static str,
}

const FRONTEND_ROUTES: &[FrontendRouteContract] = &[
    FrontendRouteContract {
        label: "startup markers",
        source: "app/javascript/mastodon/actions/markers.ts",
        evidence: "`/api/v1/markers`",
        path: "/api/v1/markers",
        method: ApiMethod::Get,
        support: ApiRouteSupport::Implemented,
        router_handler: "get(markers).post(marker_update)",
    },
    FrontendRouteContract {
        label: "startup home timeline",
        source: "app/javascript/mastodon/actions/timelines.js",
        evidence: "expandTimeline('home', '/api/v1/timelines/home'",
        path: "/api/v1/timelines/home",
        method: ApiMethod::Get,
        support: ApiRouteSupport::Implemented,
        router_handler: "get(home_timeline)",
    },
    FrontendRouteContract {
        label: "startup grouped notifications",
        source: "app/javascript/mastodon/api/notifications.ts",
        evidence: "url: '/api/v2/notifications'",
        path: "/api/v2/notifications",
        method: ApiMethod::Get,
        support: ApiRouteSupport::Implemented,
        router_handler: "get(grouped_notifications)",
    },
    FrontendRouteContract {
        label: "disabled startup translation probe",
        source: "app/javascript/mastodon/api/instance.ts",
        evidence: "'v1/instance/translation_languages'",
        path: "/api/v1/instance/translation_languages",
        method: ApiMethod::Get,
        support: ApiRouteSupport::DisabledResponse,
        router_handler: "get(translation_languages)",
    },
    FrontendRouteContract {
        label: "home announcements",
        source: "app/javascript/mastodon/actions/announcements.js",
        evidence: "api().get('/api/v1/announcements')",
        path: "/api/v1/announcements",
        method: ApiMethod::Get,
        support: ApiRouteSupport::DisabledResponse,
        router_handler: "get(announcements)",
    },
    FrontendRouteContract {
        label: "hashtag column search",
        source: "app/javascript/mastodon/features/hashtag_timeline/containers/column_settings_container.js",
        evidence: "api().get('/api/v2/search', { params: { q: value, type: 'hashtags' } })",
        path: "/api/v2/search",
        method: ApiMethod::Get,
        support: ApiRouteSupport::Implemented,
        router_handler: "get(search_v2)",
    },
];

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn mastodon_source() -> PathBuf {
    repository_root().join("target/mastodon-v4.6.5")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read pinned source {}: {error}", path.display()))
}

fn without_whitespace(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
fn pinned_frontend_routes_are_explicitly_supported_and_routed() {
    let source = mastodon_source();
    assert!(
        source.join(".git").exists(),
        "missing pinned Mastodon source; run `mise run fixture-obtain`"
    );
    let ui = read(&source.join("app/javascript/mastodon/features/ui/index.jsx"));
    for dispatch in [
        "fetchMarkers()",
        "expandHomeTimeline()",
        "fetchNotifications()",
        "fetchServerTranslationLanguages()",
        "checkAnnualReport()",
    ] {
        assert!(
            ui.contains(dispatch),
            "pinned startup dispatch drifted: {dispatch}"
        );
    }
    let home = read(&source.join("app/javascript/mastodon/features/home_timeline/index.jsx"));
    assert!(home.contains("dispatch(fetchAnnouncements())"));
    let annual_report =
        read(&source.join("app/javascript/mastodon/reducers/slices/annual_report.ts"));
    assert!(annual_report.contains("if (!year || !me || !needsStateRefresh)"));

    let router_source = without_whitespace(&read(&repository_root().join("src/web.rs")));
    for contract in FRONTEND_ROUTES {
        let frontend_source = read(&source.join(contract.source));
        assert!(
            frontend_source.contains(contract.evidence),
            "{} frontend evidence drifted at {}",
            contract.label,
            contract.source
        );
        let inventory = API_ROUTE_INVENTORY
            .iter()
            .find(|route| route.path == contract.path && route.method == contract.method)
            .unwrap_or_else(|| panic!("{} is absent from the API inventory", contract.label));
        assert_eq!(
            inventory.support, contract.support,
            "{} support drifted",
            contract.label
        );
        let route = without_whitespace(&format!(
            ".route(\"{}\",{}",
            contract.path, contract.router_handler
        ));
        assert!(
            router_source.contains(&route),
            "{} is inventoried but missing from the production router",
            contract.label
        );
    }

    let hashtag_source = read(&source.join(FRONTEND_ROUTES.last().unwrap().source));
    for shape in ["response.data.hashtags", "tag.name"] {
        assert!(
            hashtag_source.contains(shape),
            "hashtag response shape drifted: {shape}"
        );
    }
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
fn pinned_rails_uri_only_create_contract_dereferences_before_materializing_notes() {
    let source = mastodon_source();
    let activity = read(&source.join("app/lib/activitypub/activity.rb"));
    let create = read(&source.join("app/lib/activitypub/activity/create.rb"));
    let create_spec = read(&source.join("spec/lib/activitypub/activity/create_spec.rb"));
    assert!(activity.contains("return unless @object.is_a?(String)"));
    assert!(activity.contains("@object = dereferencer.object unless dereferencer.object.nil?"));
    assert!(
        create.contains(
            "def perform\n    @account.schedule_refresh_if_stale!\n\n    dereference_object!\n\n    create_status\n  end"
        ),
        "Rails Create must dereference URI objects immediately before materialization"
    );
    for evidence in [
        "context 'when object URI uses bearcaps'",
        "stub_request(:get, object_json[:id])",
        "expect(status).to_not be_nil",
        "text: 'Lorem ipsum'",
    ] {
        assert!(
            create_spec.contains(evidence),
            "pinned Rails URI-only Create behavior drifted: {evidence}"
        );
    }
}
