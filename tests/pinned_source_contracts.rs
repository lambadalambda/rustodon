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
        label: "delayed authenticated instance refresh",
        source: "app/javascript/mastodon/api/instance.ts",
        evidence: "apiRequestGet<ApiInstanceJSON>('v2/instance')",
        path: "/api/v2/instance",
        method: ApiMethod::Get,
        support: ApiRouteSupport::Implemented,
        router_handler: "get(instance_v2)",
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
        support: ApiRouteSupport::Implemented,
        router_handler: "get(announcements)",
    },
    FrontendRouteContract {
        label: "poll refresh",
        source: "app/javascript/mastodon/api/polls.ts",
        evidence: "apiRequestGet<ApiPollJSON>(`v1/polls/${pollId}`)",
        path: "/api/v1/polls/{id}",
        method: ApiMethod::Get,
        support: ApiRouteSupport::Implemented,
        router_handler: "get(poll_show)",
    },
    FrontendRouteContract {
        label: "poll vote",
        source: "app/javascript/mastodon/api/polls.ts",
        evidence: "apiRequestPost<ApiPollJSON>(`v1/polls/${pollId}/votes`",
        path: "/api/v1/polls/{id}/votes",
        method: ApiMethod::Post,
        support: ApiRouteSupport::Implemented,
        router_handler: "post(poll_vote)",
    },
    FrontendRouteContract {
        label: "quote list",
        source: "app/javascript/mastodon/api/interactions.ts",
        evidence: "url: url ?? `/api/v1/statuses/${statusId}/quotes`",
        path: "/api/v1/statuses/{id}/quotes",
        method: ApiMethod::Get,
        support: ApiRouteSupport::Implemented,
        router_handler: "get(status_quotes)",
    },
    FrontendRouteContract {
        label: "quote interaction policy",
        source: "app/javascript/mastodon/api/statuses.ts",
        evidence: "`v1/statuses/${statusId}/interaction_policy`",
        path: "/api/v1/statuses/{id}/interaction_policy",
        method: ApiMethod::Put,
        support: ApiRouteSupport::Implemented,
        router_handler: "put(status_interaction_policy_update)",
    },
    FrontendRouteContract {
        label: "quote revoke",
        source: "app/javascript/mastodon/api/interactions.ts",
        evidence: "`v1/statuses/${quotedStatusId}/quotes/${statusId}/revoke`",
        path: "/api/v1/statuses/{quoted_status_id}/quotes/{id}/revoke",
        method: ApiMethod::Post,
        support: ApiRouteSupport::Implemented,
        router_handler: "post(revoke_quote)",
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
        "fetchServer()",
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

    let poll_actions = read(&source.join("app/javascript/mastodon/actions/polls.ts"));
    assert!(poll_actions.contains("apiPollVote(pollId, choices)"));
    assert!(poll_actions.contains("apiGetPoll(pollId)"));
    assert_eq!(
        poll_actions.matches("importFetchedPoll({ poll })").count(),
        2
    );
    let poll_api = read(&source.join("app/javascript/mastodon/api/polls.ts"));
    assert!(
        poll_api.contains(
            "apiRequestPost<ApiPollJSON>(`v1/polls/${pollId}/votes`, {\n    choices,\n  })"
        )
    );
    let compose = read(&source.join("app/javascript/mastodon/actions/compose.js"));
    assert!(compose.contains("poll: getState().getIn(['compose', 'poll'], null)"));

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
fn pinned_poll_frontend_contract_covers_compose_vote_and_refresh() {
    let source = mastodon_source();
    let compose =
        read(&source.join("app/javascript/mastodon/features/compose/components/poll_form.jsx"));
    for evidence in [
        "compose-form__poll",
        "poll__option editable",
        "compose-form__poll__select__value",
        "placeholder={intl.formatMessage(messages.option_placeholder, { number: index + 1 })}",
        "{ value: 300",
        "{ value: 604800",
        "defaultMessage: 'Option {number}'",
    ] {
        assert!(
            compose.contains(evidence),
            "poll composer drifted: {evidence}"
        );
    }
    let compose_form =
        read(&source.join("app/javascript/mastodon/features/compose/components/compose_form.jsx"));
    for evidence in [
        "<AutosuggestTextarea",
        "<div className='compose-form__submit'>",
        "type='submit'",
    ] {
        assert!(
            compose_form.contains(evidence),
            "poll composer submit selector drifted: {evidence}"
        );
    }
    let button =
        read(&source.join("app/javascript/mastodon/features/compose/components/poll_button.jsx"));
    for evidence in [
        "Add a poll",
        "Remove poll",
        "compose-form__poll-button-icon",
    ] {
        assert!(button.contains(evidence), "poll button drifted: {evidence}");
    }
    let poll = read(&source.join("app/javascript/mastodon/components/poll.tsx"));
    for evidence in [
        "name='vote-options'",
        "data-index={index}",
        "<label",
        "className={classNames('poll__option'",
        "const voteDisabled =",
        "Object.values(selected).every((item) => !item)",
        "onChange={handleOptionChange}",
        "disabled={voteDisabled}",
        "className='button button-secondary'",
        "className='poll__link'",
        "poll__voted",
        "defaultMessage='Vote'",
        "defaultMessage='Refresh'",
    ] {
        assert!(
            poll.contains(evidence),
            "poll interaction drifted: {evidence}"
        );
    }
    let status_page = read(&source.join("app/javascript/mastodon/features/status/index.jsx"));
    for evidence in ["detailed-status__wrapper", "<DetailedStatus"] {
        assert!(
            status_page.contains(evidence),
            "poll permalink wrapper drifted: {evidence}"
        );
    }
    let detailed_status = read(
        &source.join("app/javascript/mastodon/features/status/components/detailed_status.tsx"),
    );
    for evidence in [
        "classNames('detailed-status'",
        "className='detailed-status__datetime'",
        "href={`/@${status.getIn(['account', 'acct'])}/${status.get('id')}`}",
    ] {
        assert!(
            detailed_status.contains(evidence),
            "poll permalink identity selector drifted: {evidence}"
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

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
#[allow(clippy::too_many_lines)]
fn pinned_timeline_stream_protocol_covers_every_bundled_channel_and_lifecycle() {
    let source = mastodon_source();
    let frontend = read(&source.join("app/javascript/mastodon/actions/streaming.js"));
    for evidence in [
        "`public:local${onlyMedia ? ':media' : ''}`",
        "`public${onlyRemote ? ':remote' : ''}${onlyMedia ? ':media' : ''}`",
        "`hashtag${onlyLocal ? ':local' : ''}`",
        "{ tag: tagName }",
        "'list', { list: listId }",
        "case 'update':",
        "case 'status.update':",
        "case 'delete':",
        "fillPublicTimelineGaps",
        "fillCommunityTimelineGaps",
        "fillListTimelineGaps",
    ] {
        assert!(
            frontend.contains(evidence),
            "pinned frontend streaming contract drifted: {evidence}"
        );
    }

    for evidence in [
        "dispatch(updateTimeline(timelineId, JSON.parse(data.payload), { accept: options.accept, bogusQuotePolicy }))",
        "dispatch(updateStatus(JSON.parse(data.payload), { bogusQuotePolicy }))",
        "dispatch(deleteFromTimelines(data.payload))",
    ] {
        assert!(
            frontend.contains(evidence),
            "pinned streaming reducer dispatch drifted: {evidence}"
        );
    }
    let timeline_actions = read(&source.join("app/javascript/mastodon/actions/timelines.js"));
    for evidence in [
        "export const TIMELINE_UPDATE  = 'TIMELINE_UPDATE'",
        "dispatch(importFetchedStatus(status, { bogusQuotePolicy }))",
        "type: TIMELINE_UPDATE",
        "dispatch(timelineDelete({ statusId: id, accountId, references, reblogOf }))",
    ] {
        assert!(
            timeline_actions.contains(evidence),
            "pinned timeline action/reducer chain drifted: {evidence}"
        );
    }
    let status_actions = read(&source.join("app/javascript/mastodon/actions/statuses.js"));
    assert!(status_actions.contains(
        "export const updateStatus = (status, { bogusQuotePolicy }) => dispatch =>\n  dispatch(importFetchedStatus(status, { bogusQuotePolicy }))"
    ));
    let importer = read(&source.join("app/javascript/mastodon/actions/importer/index.js"));
    assert!(importer.contains("export const STATUS_IMPORT   = 'STATUS_IMPORT'"));
    assert!(importer.contains("return { type: STATUS_IMPORT, status }"));
    let typed_timeline_actions =
        read(&source.join("app/javascript/mastodon/actions/timelines_typed.ts"));
    assert!(typed_timeline_actions.contains("}>('timelines/delete')"));

    let timelines = read(&source.join("app/javascript/mastodon/reducers/timelines.js"));
    assert!(
        !timelines.contains("STATUS_IMPORT"),
        "status.update imports must not remove the status from another open timeline"
    );
    assert!(
        timelines.contains("state.keySeq().forEach(timeline =>"),
        "delete remains intentionally global across open timelines"
    );
    for evidence in [
        "case TIMELINE_UPDATE:",
        "timelineDelete.match(action)",
        "pendingItems'], ImmutableList()).includes(status.get('id'))",
        "items'], ImmutableList()).includes(status.get('id'))",
        "const includesId = ids.includes(status.get('id'))",
        "const helper = list => list.filterNot(item => item === id)",
    ] {
        assert!(
            timelines.contains(evidence),
            "pinned timeline replay idempotence drifted: {evidence}"
        );
    }
    let statuses = read(&source.join("app/javascript/mastodon/reducers/statuses.js"));
    assert!(statuses.contains("case STATUS_IMPORT:"));
    assert!(statuses.contains("state.set(status.id, fromJS(status))"));
    assert!(statuses.contains("return state.delete(id)"));
    let hashtag_connection = frontend
        .split("export const connectHashtagStream")
        .nth(1)
        .and_then(|source| source.split("export const connectDirectStream").next())
        .expect("pinned hashtag stream function");
    assert!(hashtag_connection.contains("{ tag: tagName }, { accept })"));
    assert!(
        !hashtag_connection.contains("fillGaps"),
        "hashtag reconnect unexpectedly gained REST gap filling; retained replay contract changed"
    );

    let client = read(&source.join("app/javascript/mastodon/stream.js"));
    assert!(client.contains("params.tag === streamIdentifier"));
    assert!(client.contains("params.list === streamIdentifier"));
    assert!(client.contains("type: 'subscribe'"));
    assert!(client.contains("type: 'unsubscribe'"));

    let server = read(&source.join("streaming/index.js"));
    for channel in [
        "case 'public':",
        "case 'public:media':",
        "case 'public:local':",
        "case 'public:local:media':",
        "case 'public:remote':",
        "case 'public:remote:media':",
        "case 'hashtag':",
        "case 'hashtag:local':",
        "case 'list':",
    ] {
        assert!(server.contains(channel), "pinned server omitted {channel}");
    }
    for error in [
        "Missing tag name parameter",
        "Missing list name parameter",
        "Not authorized to stream this list",
        "Unknown stream type",
        "Access token does not have the required scopes",
    ] {
        assert!(
            server.contains(error),
            "pinned server error drifted: {error}"
        );
    }
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
#[allow(clippy::too_many_lines)]
fn pinned_poll_backend_contract_covers_policy_delivery_refresh_and_expiry() {
    let source = mastodon_source();
    let policy = read(&source.join("app/policies/poll_policy.rb"));
    for evidence in [
        "StatusPolicy.new(current_account, record.status).show?",
        "!current_account.blocking?(record.account)",
        "!record.account.blocking?(current_account)",
    ] {
        assert!(policy.contains(evidence), "poll policy drifted: {evidence}");
    }

    let vote = read(&source.join("app/services/vote_service.rb"));
    for evidence in [
        "ApplicationRecord.transaction do",
        "@choices.each do |choice|",
        "ActivityPub::DistributePollUpdateWorker.perform_in(3.minutes",
        "PollExpirationNotifyWorker.perform_at(@poll.expires_at + 5.minutes",
        "@votes.each do |vote|",
        "ActivityPub::DeliveryWorker.perform_async(",
    ] {
        assert!(
            vote.contains(evidence),
            "poll vote service drifted: {evidence}"
        );
    }

    let refresh = read(&source.join("app/services/activitypub/fetch_remote_poll_service.rb"));
    assert!(refresh.contains("return unless supported_context?(json)"));
    assert!(
        refresh
            .contains("ActivityPub::ProcessStatusUpdateService.new.call(poll.status, json, json)")
    );
    let status_update =
        read(&source.join("app/services/activitypub/process_status_update_service.rb"));
    for evidence in [
        "return @status if !expected_type? || already_updated_more_recently?",
        "@status_parser.edited_at > @status.edited_at",
        "update_poll!",
        "return unless allow_significant_changes",
        "previous_poll.destroy!",
        "@status.poll_id = nil",
        "return unless poll.present? && poll.expires_at.present? && poll.votes.exists?",
        "return if @previous_expires_at&.past?",
        "PollExpirationNotifyWorker.remove_from_scheduled(poll.id) if @previous_expires_at.present? && @previous_expires_at > poll.expires_at",
        "PollExpirationNotifyWorker.perform_at(poll.expires_at + 5.minutes, poll.id)",
    ] {
        assert!(
            status_update.contains(evidence),
            "poll-removal update contract drifted: {evidence}"
        );
    }
    let status_update_spec =
        read(&source.join("spec/services/activitypub/process_status_update_service_spec.rb"));
    for evidence in [
        "context 'when originally with a poll'",
        "it 'removes poll and records media change in edit'",
        "expect(status.reload.poll).to be_nil",
        "context 'with an implicit update of a poll that has already expired'",
        "it 'does not re-trigger notifications'",
        ".to_not enqueue_sidekiq_job(PollExpirationNotifyWorker)",
    ] {
        assert!(
            status_update_spec.contains(evidence),
            "poll-removal regression contract drifted: {evidence}"
        );
    }

    let local_notification = read(&source.join("app/workers/local_notification_worker.rb"));
    for evidence in [
        "if %w(update quoted_update collection_update).include?(type)",
        "Notification.where(account: receiver, activity: activity, type: type).in_batches.delete_all",
        "elsif Notification.where(account: receiver, activity: activity, type: type).any?",
        "NotifyService.new.call(receiver, type || activity_class_name.underscore, activity, **options.symbolize_keys)",
    ] {
        assert!(
            local_notification.contains(evidence),
            "local notification idempotence/dismissal contract drifted: {evidence}"
        );
    }

    let expiry = read(&source.join("app/workers/poll_expiration_notify_worker.rb"));
    for evidence in [
        "sidekiq_options lock: :until_executing",
        "return if missing_expiration?",
        "requeue! && return if not_due_yet?",
        "notify_remote_voters_and_owner! if @poll.local?",
        "notify_local_voters!",
        "@poll.expires_at + 5.minutes",
        "ActivityPub::DistributePollUpdateWorker.perform_async(@poll.status.id)",
        "LocalNotificationWorker.perform_async(@poll.account_id, @poll.id, 'Poll', 'poll')",
        "@poll.voters.merge(Account.local).select(:id).find_in_batches",
        "[account.id, @poll.id, 'Poll', 'poll']",
        "rescue ActiveRecord::RecordNotFound",
    ] {
        assert!(
            expiry.contains(evidence),
            "poll expiry worker drifted: {evidence}"
        );
    }

    let expiry_spec = read(&source.join("spec/workers/poll_expiration_notify_worker_spec.rb"));
    for evidence in [
        "expect(ActivityPub::DistributePollUpdateWorker).to have_enqueued_sidekiq_job(poll.status.id)",
        "expect(LocalNotificationWorker).to have_enqueued_sidekiq_job(poll.account.id, poll.id, 'Poll', 'poll')",
        "expect(LocalNotificationWorker).to have_enqueued_sidekiq_job(poll_vote.account.id, poll.id, 'Poll', 'poll')",
        "expect(ActivityPub::DistributePollUpdateWorker).to_not have_enqueued_sidekiq_job(poll.status.id)",
        "expect(LocalNotificationWorker).to_not have_enqueued_sidekiq_job(poll.account.id, poll.id, 'Poll', 'poll')",
    ] {
        assert!(
            expiry_spec.contains(evidence),
            "poll expiry local/remote effect contract drifted: {evidence}"
        );
    }

    let note = read(&source.join("app/serializers/activitypub/note_serializer.rb"));
    assert!(
        note.contains("context_extensions :atom_uri, :conversation, :sensitive, :voters_count")
    );
    assert!(note.contains("class CustomEmojiSerializer < ActivityPub::EmojiSerializer; end"));
    let emoji = read(&source.join("app/serializers/activitypub/emoji_serializer.rb"));
    assert!(emoji.contains("class ActivityPub::EmojiSerializer < ActivityPub::Serializer"));
    assert!(emoji.contains("context_extensions :emoji"));
    let context = read(&source.join("app/helpers/context_helper.rb"));
    assert!(
        context.contains(
            "emoji: { 'toot' => 'http://joinmastodon.org/ns#', 'Emoji' => 'toot:Emoji' }"
        )
    );
    assert!(context.contains(
        "voters_count: { 'toot' => 'http://joinmastodon.org/ns#', 'votersCount' => 'toot:votersCount' }"
    ));
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
fn pinned_quote_frontend_contract_covers_compose_submit_and_reload_rendering() {
    let source = mastodon_source();
    let boost_button =
        read(&source.join("app/javascript/mastodon/components/status/boost_button.tsx"));
    assert!(boost_button.contains("dispatch(quoteComposeById(statusId));"));

    let typed_compose = read(&source.join("app/javascript/mastodon/actions/compose_typed.ts"));
    for evidence in [
        "dispatch(quoteComposeByStatus(status));",
        "dispatch(quoteCompose(status));",
    ] {
        assert!(
            typed_compose.contains(evidence),
            "quote compose action drifted: {evidence}"
        );
    }

    let reducer = read(&source.join("app/javascript/mastodon/reducers/compose.js"));
    assert!(reducer.contains(".set('quoted_status_id', isDirect ? null : status.get('id'))"));
    let compose_form =
        read(&source.join("app/javascript/mastodon/features/compose/components/compose_form.jsx"));
    assert!(compose_form.contains("<ComposeQuotedStatus />"));
    let quoted_post =
        read(&source.join("app/javascript/mastodon/features/compose/components/quoted_post.tsx"));
    assert!(quoted_post.contains("['quoted_status', quotedStatusId]"));

    let submit = read(&source.join("app/javascript/mastodon/actions/compose.js"));
    for evidence in [
        "url: statusId === null ? '/api/v1/statuses' : `/api/v1/statuses/${statusId}`",
        "method: statusId === null ? 'post' : 'put'",
        "quoted_status_id: getState().getIn(['compose', 'quoted_status_id']),",
        "'Idempotency-Key': getState().getIn(['compose', 'idempotencyKey']),",
    ] {
        assert!(
            submit.contains(evidence),
            "quote request contract drifted: {evidence}"
        );
    }

    let interactions = read(&source.join("app/javascript/mastodon/api/interactions.ts"));
    assert!(interactions.contains("apiRequestPost<ApiStatusJSON>(\n    `v1/statuses/${quotedStatusId}/quotes/${statusId}/revoke`,"));
    assert!(
        interactions
            .contains("method: 'GET',\n    url: url ?? `/api/v1/statuses/${statusId}/quotes`,")
    );
    let interaction_actions =
        read(&source.join("app/javascript/mastodon/actions/interactions_typed.ts"));
    assert!(interaction_actions.contains("apiRevokeQuote(quotedStatusId, statusId)"));
    assert!(interaction_actions.contains("apiGetQuotes(statusId, next)"));
    let revoke_modal = read(&source.join(
        "app/javascript/mastodon/features/ui/components/confirmation_modals/revoke_quote.tsx",
    ));
    assert!(revoke_modal.contains("dispatch(revokeQuote({ quotedStatusId, statusId }))"));
    let quotes_view = read(&source.join("app/javascript/mastodon/features/quotes/index.tsx"));
    assert!(quotes_view.contains("dispatch(fetchQuotes({ statusId }))"));

    let statuses_api = read(&source.join("app/javascript/mastodon/api/statuses.ts"));
    assert!(statuses_api.contains(
        "apiRequestPut<ApiStatusJSON>(\n    `v1/statuses/${statusId}/interaction_policy`,"
    ));
    assert!(statuses_api.contains("quote_approval_policy: policy"));
    let status_actions = read(&source.join("app/javascript/mastodon/actions/statuses_typed.ts"));
    assert!(status_actions.contains("apiSetQuotePolicy(statusId, policy)"));
    let status_container =
        read(&source.join("app/javascript/mastodon/containers/status_container.jsx"));
    assert!(status_container.contains("setStatusQuotePolicy({ policy: quotePolicy, statusId })"));

    // Reload has no special restore path: the fetched REST quote is normalized
    // into IDs and rendered by the ordinary status import/render chain.
    let normalizer = read(&source.join("app/javascript/mastodon/actions/importer/normalizer.js"));
    assert!(normalizer.contains(
        "quoted_status: status.quote.quoted_status?.id ?? status.quote?.quoted_status_id,"
    ));
    let rendered = read(&source.join("app/javascript/mastodon/components/status_quoted.tsx"));
    for evidence in [
        "<div className='status__quote'>",
        "id={quotedStatusId}",
        "const reblogId = status?.get('reblog') as string | undefined;",
        "return reblogId ? state.statuses.get(reblogId) : status;",
    ] {
        assert!(
            rendered.contains(evidence),
            "quote rendering contract drifted: {evidence}"
        );
    }
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
#[allow(clippy::too_many_lines)]
fn pinned_quote_backend_contract_covers_create_policy_and_failure_boundaries() {
    let source = mastodon_source();
    let controller = read(&source.join("app/controllers/api/v1/statuses_controller.rb"));
    for evidence in [
        "before_action :set_quoted_status, only:    [:create]",
        "quoted_status: @quoted_status,",
        "idempotency: request.headers['Idempotency-Key'],",
        "Status.find(status_params[:quoted_status_id])&.proper",
        "authorize(@quoted_status, :quote?) if @quoted_status.present?",
        ":quoted_status_id,",
    ] {
        assert!(
            controller.contains(evidence),
            "quote controller contract drifted: {evidence}"
        );
    }
    let status = read(&source.join("app/models/status.rb"));
    assert!(status.contains("reblog? ? reblog : self"));

    let post = read(&source.join("app/services/post_status_service.rb"));
    for evidence in [
        "@visibility   = :private if @quoted_status&.private_visibility? && %i(public unlisted).include?(@visibility&.to_sym)",
        "attach_quote!(@status)",
        "status.quote = Quote.create(quoted_status: @quoted_status, status: status)",
        "status.quote.ensure_quoted_access",
        "status.quote.accept! if @quoted_status.local? && StatusPolicy.new(@status.account, @quoted_status).quote?",
        "ActivityPub::QuoteRequestWorker.perform_async(@status.quote.id)",
        "return if @quoted_status.nil? || @visibility.to_sym != :direct",
        "status.errors.add(:base, I18n.t('statuses.errors.quoted_user_not_mentioned'))",
        "with_redis_lock(\"idempotency:lock:status:#{@account.id}:#{@options[:idempotency]}\") do",
        "return idempotency_duplicate if idempotency_duplicate?",
    ] {
        assert!(
            post.contains(evidence),
            "quote create contract drifted: {evidence}"
        );
    }

    let interaction_policy_controller =
        read(&source.join("app/controllers/api/v1/statuses/interaction_policies_controller.rb"));
    for evidence in [
        "doorkeeper_authorize! :write, :'write:statuses'",
        "authorize @status, :update?",
        "@status.update!(quote_approval_policy: quote_approval_policy)",
        "broadcast_updates! if @status.quote_approval_policy_previously_changed?",
        "'skip_notifications' => true",
        "ActivityPub::StatusUpdateDistributionWorker.perform_async",
    ] {
        assert!(
            interaction_policy_controller.contains(evidence),
            "interaction-policy controller contract drifted: {evidence}"
        );
    }
    let interaction_policy_params =
        read(&source.join("app/controllers/concerns/api/interaction_policies_concern.rb"));
    for evidence in [
        "when 'public'",
        "when 'followers'",
        "when 'nobody'",
        "raise ActiveRecord::RecordInvalid",
    ] {
        assert!(
            interaction_policy_params.contains(evidence),
            "interaction-policy validation contract drifted: {evidence}"
        );
    }

    let policy = read(&source.join("app/policies/status_policy.rb"));
    for evidence in [
        "show? && !blocking_author? && record.quote_policy_for_account(current_account) != :denied",
        "current_account.blocking?(author)",
        "current_account.blocked_by?(author)",
    ] {
        assert!(
            policy.contains(evidence),
            "quote policy drifted: {evidence}"
        );
    }
    let interaction =
        read(&source.join("app/models/concerns/status/interaction_policy_concern.rb"));
    for evidence in [
        "return :denied if other_account.nil? || direct_visibility? || reblog?",
        "return :automatic if account_id == other_account.id",
        "return :automatic if automatic_policy.public?",
        "return :manual if manual_policy.public?",
    ] {
        assert!(
            interaction.contains(evidence),
            "quote policy drifted: {evidence}"
        );
    }

    let request_spec = read(&source.join("spec/requests/api/v1/statuses_spec.rb"));
    for evidence in [
        "context 'with a self-quote post' do",
        "expect(response.parsed_body[:quote]).to be_present",
        "expect(response.parsed_body[:quote][:quoted_status][:id]).to eq quoted_status.id.to_s",
        "context 'with a quote to a non-mentioned user in a Private Mention' do",
        "expect(response).to have_http_status(422)",
        "context 'when the quoter is blocked by the quotee' do",
        "context 'when the quotee is blocked by the quoter' do",
    ] {
        assert!(
            request_spec.contains(evidence),
            "quote request regression drifted: {evidence}"
        );
    }

    let service_spec = read(&source.join("spec/services/post_status_service_spec.rb"));
    for evidence in [
        "it 'returns existing status when used twice with idempotency key' do",
        "expect(status2.id).to eq status1.id",
        ".to enqueue_sidekiq_job(ActivityPub::QuoteRequestWorker)",
    ] {
        assert!(
            service_spec.contains(evidence),
            "quote service regression drifted: {evidence}"
        );
    }
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
#[allow(clippy::too_many_lines)]
fn pinned_quote_federation_and_output_contract_covers_lifecycle_effects() {
    let source = mastodon_source();
    let create = read(&source.join("app/lib/activitypub/activity/create.rb"));
    for evidence in [
        "process_quote",
        "@status = Status.create!(@params.merge(quote: @quote))",
        "@quote = Quote.new(account: @account, approval_uri: nil, legacy: @status_parser.legacy_quote?",
        "ActivityPub::VerifyQuoteService.new.call(@quote, @quote_approval_uri",
        "ActivityPub::RefetchAndVerifyQuoteWorker.perform_in",
    ] {
        assert!(
            create.contains(evidence),
            "remote quote create drifted: {evidence}"
        );
    }

    let verify = read(&source.join("app/services/activitypub/verify_quote_service.rb"));
    for evidence in [
        "return if fast_track_approval! || @approval_uri.blank?",
        "return quote.reject! if @json.nil?",
        "return unless matching_type? && matching_quote_uri?",
        "return unless matching_quoted_post? && matching_quoted_author?",
        "quote.accept!(approval_uri: @approval_uri)",
        "if @quote.account_id == @quote.quoted_account_id",
        "@quote.update(quoted_status: status) if status.present? && !status.reblog?",
    ] {
        assert!(
            verify.contains(evidence),
            "quote verification drifted: {evidence}"
        );
    }

    let quote_request = read(&source.join("app/lib/activitypub/activity/quote_request.rb"));
    for evidence in [
        "!quoted_status.distributable? || quoted_status.reblog?",
        "if StatusPolicy.new(@account, quoted_status).quote?",
        "status.quote.accept!",
        "LocalNotificationWorker.perform_async(quoted_status.account_id, status.quote.id, 'Quote', 'quote')",
        "DistributionWorker.perform_async(status.id, { 'update' => true, 'skip_notifications' => true })",
    ] {
        assert!(
            quote_request.contains(evidence),
            "QuoteRequest drifted: {evidence}"
        );
    }

    let accept = read(&source.join("app/lib/activitypub/activity/accept.rb"));
    for evidence in [
        "quote.quoted_account != @account || !quote.status.local? || !quote.pending?",
        "quote.update!(state: :accepted, approval_uri: approval_uri)",
        "ActivityPub::StatusUpdateDistributionWorker.perform_async",
    ] {
        assert!(
            accept.contains(evidence),
            "quote Accept drifted: {evidence}"
        );
    }
    let reject = read(&source.join("app/lib/activitypub/activity/reject.rb"));
    assert!(
        reject.contains("return unless quote.quoted_account == @account && quote.status.local?")
    );
    assert!(reject.contains("quote.reject!"));

    let update = read(&source.join("app/services/activitypub/process_status_update_service.rb"));
    for evidence in [
        "update_quote!",
        "@status.quote.destroy!",
        "RevokeQuoteService.new.call(@status.quote)",
        "def update_quote_approval!",
    ] {
        assert!(
            update.contains(evidence),
            "quote update drifted: {evidence}"
        );
    }
    let update_spec =
        read(&source.join("spec/services/activitypub/process_status_update_service_spec.rb"));
    for evidence in [
        "when the status removes a verified quote through an implicit update",
        "it 'does not remove the quote' do",
        "when the status removes a verified quote through an explicit update",
        "to change { status.reload.quote }.to(nil)",
    ] {
        assert!(
            update_spec.contains(evidence),
            "quote update regression drifted: {evidence}"
        );
    }

    let delete = read(&source.join("app/lib/activitypub/activity/delete.rb"));
    for evidence in [
        "Quote.find_by(approval_uri: object_uri, quoted_account: @account, state: [:pending, :accepted])",
        "@quote.reject!",
        "DistributionWorker.perform_async(@quote.status_id, { 'update' => true }) if @quote.status.present?",
    ] {
        assert!(
            delete.contains(evidence),
            "quote authorization Delete drifted: {evidence}"
        );
    }
    let revoke = read(&source.join("app/services/revoke_quote_service.rb"));
    for evidence in [
        "@quote.reject!",
        "distribute_update!",
        "distribute_stamp_deletion!",
        "ActivityPub::DeliveryWorker.push_bulk(inboxes, limit: 1_000)",
    ] {
        assert!(
            revoke.contains(evidence),
            "quote revoke drifted: {evidence}"
        );
    }

    let quote = read(&source.join("app/models/quote.rb"));
    for evidence in [
        "after_create_commit :increment_counter_caches!",
        "after_destroy_commit :decrement_counter_caches!",
        "after_update_commit :update_counter_caches!",
        "quoted_status&.increment_count!(:quotes_count)",
        "quoted_status&.decrement_count!(:quotes_count)",
    ] {
        assert!(
            quote.contains(evidence),
            "quote counter drifted: {evidence}"
        );
    }
    let note = read(&source.join("app/serializers/activitypub/note_serializer.rb"));
    for evidence in [
        "attribute :quote, if: :quote?",
        "attribute :quote, key: :_misskey_quote, if: :serializable_quote?",
        "attribute :quote, key: :quote_uri, if: :serializable_quote?",
        "attribute :quote_authorization, if: :quote_authorization?",
    ] {
        assert!(
            note.contains(evidence),
            "quote Note output drifted: {evidence}"
        );
    }
    let rest = read(&source.join("app/serializers/rest/status_serializer.rb"));
    assert!(rest.contains("has_one :quote, key: :quote, serializer: REST::QuoteSerializer"));
    assert!(rest.contains(":favourites_count, :quotes_count, :edited_at"));
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
fn pinned_local_hashtag_controls_contract() {
    let source = mastodon_source();
    let controller = read(&source.join("app/controllers/api/v1/tags_controller.rb"));
    for evidence in [
        "doorkeeper_authorize! :follow, :write, :'write:follows'",
        "doorkeeper_authorize! :write, :'write:accounts'",
        "before_action :require_user!, except: :show",
        "override_rate_limit_headers :follow, family: :follows",
        "TagFollow.create_with(rate_limit: true).find_or_create_by!",
        "Tag::HASHTAG_NAME_RE.match?(params[:id])",
        "Tag.find_normalized(params[:id]) || Tag.new(name: params[:id], display_name: params[:id])",
    ] {
        assert!(controller.contains(evidence), "{evidence}");
    }
    let collection = read(&source.join("app/controllers/api/v1/featured_tags_controller.rb"));
    assert!(collection.contains("current_account.featured_tags.find(params[:id])"));
    let model = read(&source.join("app/models/featured_tag.rb"));
    for evidence in [
        "LIMIT = 10",
        "name.strip.delete_prefix('#')",
        "validates :tag_id, uniqueness: { scope: :account_id }",
        "account.statuses.distributable_visibility.tagged_with(tag)",
    ] {
        assert!(model.contains(evidence), "{evidence}");
    }
    let service = read(&source.join("app/services/create_featured_tag_service.rb"));
    assert!(service.contains("account.featured_tags.find_or_initialize_by(tag: name_or_tag)"));
    assert!(service.contains("account.featured_tags.find_or_initialize_by(name: name_or_tag)"));
    let normalizer = read(&source.join("app/lib/hashtag_normalizer.rb"));
    assert!(
        normalizer.contains("remove_invalid_characters(ascii_folding(lowercase(cjk_width(str))))")
    );
    let folding = read(&source.join("app/lib/ascii_folding.rb"));
    let rust = read(&repository_root().join("src/mastodon/repository.rs"));
    for (ruby_name, rust_name) in [
        ("NON_ASCII_CHARS", "NON_ASCII"),
        ("EQUIVALENT_ASCII_CHARS", "ASCII"),
    ] {
        let ruby_value = folding
            .lines()
            .find(|line| line.trim_start().starts_with(ruby_name))
            .unwrap()
            .split('\'')
            .nth(1)
            .unwrap();
        let rust_value = rust
            .lines()
            .find(|line| {
                line.trim_start()
                    .starts_with(&format!("const {rust_name}:"))
            })
            .unwrap()
            .split('"')
            .nth(1)
            .unwrap();
        assert_eq!(ruby_value, rust_value, "pinned folding table {ruby_name}");
    }
    let history = read(&source.join("app/models/trends/tags.rb"));
    assert!(
        history
            .contains("!status.reblog? && status.public_visibility? && !status.account.silenced?")
    );
    // Local DB history intentionally does not claim Redis retention equivalence.
    let serializer = read(&source.join("app/serializers/rest/tag_serializer.rb"));
    assert!(serializer.contains("object.id.to_s"));
    assert!(serializer.contains("attribute :following, if: :current_user?"));
    assert!(serializer.contains("attribute :featuring, if: :current_user?"));
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
fn pinned_profile_read_contract() {
    let source = mastodon_source();
    let controller = read(&source.join("app/controllers/api/v1/profiles_controller.rb"));
    assert!(controller.contains("doorkeeper_authorize! :profile, :read, :'read:accounts'"));
    assert!(controller.contains("before_action :require_user!"));
    assert!(controller.contains("@account = current_account"));
    let serializer = read(&source.join("app/serializers/rest/profile_serializer.rb"));
    let attributes = serializer
        .split("attributes ")
        .nth(1)
        .unwrap()
        .split("has_many")
        .next()
        .unwrap();
    let mut fields: Vec<_> = attributes
        .split(',')
        .map(str::trim)
        .map(|s| s.trim_start_matches(':'))
        .collect();
    fields.sort_unstable();
    let mut expected = vec![
        "id",
        "display_name",
        "note",
        "fields",
        "formatted_note",
        "formatted_fields",
        "avatar",
        "avatar_static",
        "avatar_description",
        "header",
        "header_static",
        "header_description",
        "locked",
        "bot",
        "hide_collections",
        "discoverable",
        "indexable",
        "show_media",
        "show_media_replies",
        "show_featured",
        "attribution_domains",
    ];
    expected.sort_unstable();
    assert_eq!(fields, expected);
    assert!(
        serializer.contains("has_many :featured_tags, serializer: REST::FeaturedTagSerializer")
    );
    assert!(serializer.contains("object.fields.map(&:to_h)"));
    assert!(serializer.contains(
        "object.avatar_file_name.present? ? full_asset_url(object.avatar_original_url) : nil"
    ));
    assert!(serializer.contains(
        "object.header_file_name.present? ? full_asset_url(object.header_original_url) : nil"
    ));
    let frontend = read(&source.join("app/javascript/mastodon/api/accounts.ts"));
    assert!(frontend.contains("apiRequestGet<ApiProfileJSON>('v1/profile')"));
    let reducer = read(&source.join("app/javascript/mastodon/reducers/slices/profile_edit.ts"));
    assert!(reducer.contains("fetchProfile.fulfilled"));
    assert!(reducer.contains("state.profile.featuredTags"));
    let editor = read(&source.join("app/javascript/mastodon/features/account_edit/index.tsx"));
    assert!(editor.contains("dispatch(fetchProfile())"));
    assert!(editor.contains("profile.featuredTags"));
    let tags =
        read(&source.join("app/javascript/mastodon/features/account_edit/featured_tags.tsx"));
    assert!(tags.contains("fetchProfile"));
    let route = API_ROUTE_INVENTORY
        .iter()
        .find(|route| route.path == "/api/v1/profile")
        .unwrap();
    assert_eq!(route.support, ApiRouteSupport::Implemented);
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
fn pinned_featured_editor_waits_for_delayed_instance_limit() {
    let source = mastodon_source();
    for (path, evidence) in [
        (
            "features/ui/index.jsx",
            "setTimeout(() => this.props.dispatch(fetchServer()), 3000)",
        ),
        (
            "api/instance.ts",
            "apiRequestGet<ApiInstanceJSON>('v2/instance')",
        ),
        (
            "models/server.ts",
            "createServerFromServerJSON = (obj: ApiInstanceJSON): Server => obj",
        ),
        (
            "reducers/server.ts",
            "state.server.item = createServerFromServerJSON(action.payload)",
        ),
        (
            "features/account_edit/featured_tags.tsx",
            "state.server.server.item?.configuration.accounts.max_featured_tags ?? 0",
        ),
        (
            "features/account_edit/featured_tags.tsx",
            "const canAddMoreTags = tags.length < maxTags",
        ),
        (
            "features/account_edit/featured_tags.tsx",
            "{canAddMoreTags && <AccountEditTagSearch />}",
        ),
        (
            "features/account_edit/components/tag_search.tsx",
            "dispatch(addFeaturedTags({ names: [item.name] }))",
        ),
        ("hooks/useSearchTags.ts", "defaultMessage: 'Add #{tagName}'"),
    ] {
        assert!(
            read(&source.join("app/javascript/mastodon").join(path)).contains(evidence),
            "{path}: {evidence}"
        );
    }
}

#[test]
#[ignore = "requires the read-only pinned Mastodon source checkout"]
fn pinned_daily_activity_records_and_interactive_tracking_contract() {
    let source = mastodon_source();
    let tracker = read(&source.join("app/lib/activity_tracker.rb"));
    for evidence in [
        "EXPIRE_AFTER = 6.months.seconds",
        "redis.pfadd(key, value)",
        "redis.expire(key, EXPIRE_AFTER)",
        "start_at.to_date...end_at.to_date",
        "redis.pfcount(*keys)",
    ] {
        assert!(tracker.contains(evidence), "{evidence}");
    }
    let user = read(&source.join("app/models/user.rb"));
    for evidence in [
        "prepare_new_user! if confirmed?",
        "if approved?\n      prepare_new_user!",
        "return unless confirmed?",
        "ActivityTracker.record('activity:logins', id)",
        "increment(:sign_in_count) if new_sign_in",
        "current_sign_in_at || new_current",
    ] {
        assert!(user.contains(evidence), "{evidence}");
    }
    let tracking = read(&source.join("app/controllers/concerns/user_tracking_concern.rb"));
    assert!(tracking.contains("SIGN_IN_UPDATE_FREQUENCY = 24.hours.freeze"));
    assert!(tracking.contains("before_action :update_user_sign_in"));
    assert!(tracking.contains("current_user.current_sign_in_at < SIGN_IN_UPDATE_FREQUENCY.ago"));
    let api = read(&source.join("app/controllers/api/base_controller.rb"));
    assert!(api.contains("elsif !current_user.functional?"));
    assert!(api.contains("else\n      update_user_sign_in"));
    let credentials =
        read(&source.join("app/controllers/api/v1/accounts/credentials_controller.rb"));
    assert!(credentials.contains("before_action :require_user!"));
    let presenter = read(&source.join("app/presenters/instance_presenter.rb"));
    assert!(presenter.contains("def active_user_count(num_weeks = 4)"));
    assert!(presenter.contains(".sum(num_weeks.weeks.ago)"));
    let nodeinfo = read(&source.join("app/serializers/node_info/serializer.rb"));
    assert!(nodeinfo.contains("active_user_count(24)"));
    assert!(read(&source.join("Gemfile.lock")).contains("activesupport (8.1.3)"));
}
