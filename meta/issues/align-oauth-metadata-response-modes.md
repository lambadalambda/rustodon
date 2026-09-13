# Align OAuth metadata with supported response modes

## Summary

The required pinned Rails differential, unblocked by the task-owned NAS socket,
fails because Rustodon advertises only query mode while Mastodon advertises
query, fragment and form_post. Rustodon already has response-mode behavior gates.

## Requirements

- User explicitly authorized implementing fragment and form_post after reviewing
  the query-only handler blocker. This supersedes the original metadata-only
  scope; advertising unsupported modes remains prohibited.
- Support default/query, fragment and form_post authorization-code responses,
  including consent round trips, approval, denial/error delivery and opaque state.
- Preserve redirect registration checks, session/read-auth guards, CSRF and S256
  PKCE semantics. No schema, grants, implicit flow or shared harness broadening.
- Form-post responses must safely escape fields/action, prevent caching and
  framing, and allow submission only to a validated **HTTP(S)** callback under an
  appropriately scoped CSP. Registered custom-scheme and OOB form-post
  combinations fail locally before grant/code mutation with
  `unsupported_response_mode`; do not claim support for those combinations.
  Query/fragment retain existing custom-scheme/OOB behavior. Do not weaken the
  general HTML CSP.
- Scope: `src/web.rs`, OAuth supporting modules only if necessary,
  `tests/oauth_response_modes.rs`, and this issue.

## Acceptance Criteria

- Tests first: deliberately replace fragment/form_post rejection assertions with
  supported-mode contracts, retaining unknown/malformed-mode rejection and unsafe
  callback guards. Include ordinary metadata regression and fixture coverage for
  consent, success, denial, errors, PKCE, state escaping and form-post security.
- Report phase 1 ready for parent-owned NAS RED before changing production.
- Implement behavior before advertising all three modes. Obtain independent
  read-only review before substantial handoff.
- Parent runs pinned `oauth_bearer_authentication`, response-mode fixture,
  formatting and strict lint; no local tests/builds/fmt, NAS/SSH or commits here.
- Use supplied pinned 4.6.5 evidence/extracts for upstream-specific expectations;
  do not fetch missing upstream source or weaken the differential oracle.

## Evidence

- `/srv/workspaces/rustodon-audit-green/logs/differential-task-socket.log`:
  missing `response_modes_supported[1]` fragment and `[2]` form_post.
- `src/web.rs` OAuth metadata currently emits `["query"]`.
- Follow-up from required audit gate execution; no live deployment.

## Original handler verification (superseded scope blocker)

- Owner: Alice, worktree `ilar-task-remaining-oauth-modes` at `c948975`.
- `src/web.rs:9392–9397` accepts only omitted/query response modes and rejects
  fragment/form_post before login or consent on both GET and POST. Consent
  hardcodes query mode; authorization responses append fields to the callback
  query (`oauth_consent_response`, `oauth_authorize_redirect`).
- The green `tests/oauth_response_modes.rs` fixture verifies **rejection**, not
  support, of fragment/form_post (`only_query_response_modes_are_advertised_and_accepted`).
  Its final assertion also requires discovery to advertise only `["query"]`.
- The supplied pinned 4.6.5 Rails runtime differential establishes an upstream
  metadata mismatch, not support in Rustodon. Neither prescribed pinned source
  path is available locally; no replacement was fetched.
- Independent read-only review confirmed that a metadata-only fix would advertise
  unsupported behavior and contradict the existing fixture. Additional handler
  support exceeds this issue's assigned narrow scope.
- The initial phase stopped without code/test changes. The user subsequently
  authorized implementing fragment and form_post; the requirements above now
  govern the work. Production remains query-only until tests-first NAS RED.

## Phase 1 test preparation

- Added ordinary `metadata_advertises_query_fragment_and_form_post`, using a lazy
  database pool: discovery requires no database fixture. Also guards existing
  response types/grant types against accidental broadening.
- Revised the existing fixture as
  `supported_response_modes_preserve_authorization_security`: default/query,
  fragment and form_post; absent/empty/adversarial state; consent/login return
  preservation; approval and denial; S256 persistence, failed exchange without
  consumption, successful exchange and replay rejection; CSRF, unknown/malformed
  modes and unregistered/malicious callbacks. Clients never follow callbacks.
- Form-post assertions cover an escaped validated action and fields, body-only
  delivery, auto-submit with a matching CSP hash/nonce, manual fallback, scoped
  form-action, no-store, content type and anti-framing/sniffing/referrer headers.
  These are security contracts, not claims of byte-identical Rails markup.
- Initial tests retained local invalid-scope/PKCE errors provisionally. The cached
  extract review demonstrated mismatches; parent subsequently authorized the
  tests-only correction and explicit HTTP(S) form-post boundary recorded below.
  The parent's subsequent RED results are recorded under phase 2.
- Cached OAuth source/runtime extracts were subsequently supplied by parent and
  inspected read-only; see the verified behavior and remaining decisions below.
- Planned production boundary after RED: a private response-mode type/parser,
  consent-field propagation and mode-aware response rendering in `src/web.rs`;
  reuse existing grant issuance/PKCE/session checks. No schema or grant-type
  changes are expected. Keep any callback-specific CSP local to form-post output.
- Independent read-only review found no apparent compile blocker and judged the
  draft suitable for parent NAS RED. Addressed its CSP findings: parse the consent
  directive exactly, accept callback origin/path sources rather than query URLs.
  Added single-form extraction, duplicate-field rejection, static-script target
  checking, and HTML/URL-encoded CRLF normalization in the small renderer parser.
  This is not a browser automation test or byte-identical pinned markup assertion.
- Added actual expired activation and disabled-user guards, alongside absent and
  stale sessions, for every mode on GET and POST approval/denial with valid CSRF.
  Restore the disabled fixture user before propagating request errors/assertions.
- Follow-up independent read-only review found no remaining blocking compile or
  correctness issue by inspection and approved phase 1 for parent NAS RED. Its
  small cookie-header cleanup was applied (absent session sends only CSRF cookie).
- Parent owns all execution/formatting and must observe RED before a production
  fix. No local tests/builds/fmt, NAS/SSH operations, fetches or commits performed.
  Issue remains open.

## Cached pinned OAuth verification

Parent-provided read-only extract:
`/Users/lainsoykaf/repos/rustodon/.local-instance/audit-reference/oauth-pinned/`.
`opt/mastodon/Gemfile.lock:213` confirms Doorkeeper **5.9.2**.
Below, `D` means `usr/local/bundle/gems/doorkeeper-5.9.2/`, and `M` means
`opt/mastodon/`, relative to that extract. No runtime was executed here.
An independent read-only reviewer confirmed these conclusions.

- **Validation errors:** `D/app/controllers/doorkeeper/authorizations_controller.rb`
  (7–16, 43–52, 71–92, 132–137) renders GET preauthorization failures locally as
  HTML under the default `handle_auth_errors: :render`. For authenticated POST
  with a trusted non-OOB callback, `invalid_scope` and
  `invalid_code_challenge_method` use the selected response mode: 302
  query/fragment or 200 form-post HTML. The corrected tests now assert this POST
  delivery instead of the original provisional local-JSON expectations.
- **PKCE:** `D/lib/doorkeeper/oauth/pre_authorization.rb:148–162` has no challenge
  length check and skips required-challenge validation unless `force_pkce` is
  enabled. The pinned initializer enables only S256, not `force_pkce`. A nonblank
  challenge with missing/unsupported method yields `invalid_code_challenge_method`.
  Rustodon's required public-client PKCE and challenge-length guards must not be
  relaxed as an incidental response-mode change; retain and label these as
  existing security-profile contracts, not pinned-equivalent rejection cases.
- **Mode and state:** preauthorization (130–136) treats blank/whitespace mode as
  default query. Unknown nonblank scalar mode yields `unsupported_response_mode`
  (GET local HTML; valid-client POST query-error redirect). Authentication runs
  first; Rack-shaped values are scalar-filtered by Rails, not proven equivalent
  to unknown strings. `D/lib/doorkeeper/oauth/authorization/uri_builder.rb:25–27`
  and `oauth/error_response.rb:43–48` drop blank state. Successful form-post code
  output preserves empty state via `auth.body.compact`; errors/denials do not.
  Corrected tests now cover blank modes and state asymmetry. Unknown scalar
  modes use the deliberate local fail-closed policy below, not pinned redirects.
- **OOB:** `D/lib/doorkeeper/oauth/code_response.rb:40–47` and
  `oauth/authorization/code.rb:22–24` send query/fragment success to local `show`
  with the code, without state. Controller form-post dispatch instead renders a
  form targeting the original OOB URI. Error responses are nonredirectable for
  OOB (`oauth/error_response.rb:59–60`): local JSON on POST, local HTML for GET
  preauthorization failures. Rustodon's retained OOB behavior and form-post
  refusal boundary are distinguished below.
- **Custom schemes:** `D/lib/doorkeeper/oauth/helpers/uri_checker.rb` permits
  registered hierarchical custom schemes; the pinned initializer forbids data,
  vbscript and javascript at registration. Form-post passes the original URI to
  Rails' escaping form helper with no HTTP(S)-only branch. This establishes HTML
  rendering, not browser POST support. Parent explicitly chose to refuse
  form-post for custom schemes/OOB rather than promise browser delivery.
- **Consent/denial/security distinctions:**
  `M/app/views/oauth/authorizations/new.html.haml:24–56` omits `response_mode` in
  both forms and submits denial using DELETE. Doorkeeper `destroy` calls `deny`
  without `authorizable?`; never copy that shortcut over Rustodon's callback
  validation guards. `M/app/controllers/oauth/authorizations_controller.rb:8–10`
  removes `form-action`. The gem's `form_post.html.erb` uses inline `window.onload`
  and has no manual fallback. Preserve the authorized mode-carrying consent,
  validated approval/denial, scoped CSP and fallback as intentional Rustodon
  behavior/security contracts, not byte-identical pinned UI assertions.

## Corrected tests-only boundary and error policy

Parent authorized these tests-only corrections before the observed baseline
failures recorded under phase 2.

- **Pinned-aligned delivery:** trusted-callback POST `invalid_scope` and
  `invalid_code_challenge_method` use query/fragment/form-post; blank and
  whitespace mode defaults to query; redirects and all errors omit blank state,
  while successful form-post retains empty/whitespace state. Tests include a
  missing method with a present challenge and retain meaningful absent-state
  distinctions without repairing consent fields in the test.
- **Intentional local error policy:** unknown nonblank scalar mode yields local
  HTTP 400 JSON `unsupported_response_mode` on GET and POST, authenticated or
  unauthenticated. Malformed Rack-shaped modes retain local `invalid_request`.
  This is a deliberate fail-closed policy, not a claim of Rails' GET HTML / POST
  query-error behavior. Existing callback/client and CSRF local failures remain
  unchanged. Unsupported mode handling never chooses a fallback response mode.
- **Stricter PKCE preserved:** missing public-client PKCE and malformed/short
  challenges remain local `invalid_request` without a grant. Unsupported/missing
  methods with a present valid challenge use the pinned method-error delivery.
  S256 exchange, wrong/missing verifier and replay coverage remain intact.
- **HTTP(S)-only form-post:** positive HTTP coverage complements HTTPS success,
  denial, error, escaping and precise CSP checks. Registered custom-scheme and
  OOB callbacks refuse form-post on authenticated GET and POST approval/denial,
  with valid CSRF in POST tests. Refusal is HTTP 400 local JSON:
  `{"error":"unsupported_response_mode","error_description":"form_post requires an HTTP(S) redirect_uri."}`.
  No redirect, HTML auto-post or code is returned. Full persisted grant-row
  snapshots before/after each refusal guard against insertion, code replacement
  and revocation, not only grant counts. Registration uses the same public client
  with all tested callbacks, so rejection cannot pass due to an unknown client
  or unregistered callback.
- **Existing native behavior preserved:** default/query/fragment retain registered
  custom-scheme callback delivery. OOB success retains Rustodon's local HTML code
  display, with no outbound form or location, rather than Rails' extra show
  redirect. Existing OOB denial URI delivery remains mode-aware (query/fragment),
  not mislabeled as pinned local error rendering. Approval checks persisted
  redirect/S256 fields; denial checks unchanged grant snapshots.
- Mode-preserving consent, approval/denial redirect validation, signed session
  and CSRF guards, scoped CSP and manual fallback are intentional Rustodon
  safety contracts. Do not copy upstream's dropped mode or weaker CSP/denial
  path. No claim of custom-scheme/OOB form-post support is made.

Independent read-only review of the corrected tests found no actionable
correctness/architecture issues; SQL, native/OOB expectations, grant snapshots
and helper changes were inspected but not executed.

## Phase 2: parent-observed RED and implementation

Parent reported actual NAS RED:
- Ordinary metadata regression expected `["query", "fragment", "form_post"]`,
  received `["query"]`.
- Fixture first failed query blank-state handling (`Some("")` versus `None`,
  line 716 in the parent's NAS-formatted source).
- Logs: `/srv/workspaces/rustodon-audit-green/logs/oauth-metadata-red.log` and
  `/srv/workspaces/rustodon-audit-green/logs/oauth-modes-red.log`.

Phase 2 is now implemented in `src/web.rs`; the tests snapshot is untouched:
- Private query/fragment/form-post parsing, blank-mode query default and distinct
  local errors for unknown scalars versus malformed Rack parameters.
- Session, exact callback registration and response-type validation remain ahead
  of consent/denial. Parse the callback and refuse non-HTTP(S) form-post before
  consent or grant creation, using the documented local error.
- Consent retains the selected mode. A shared escaped hidden-field helper serves
  consent and callback forms; query/fragment encode response fields separately
  while preserving the registered callback query and opaque nonblank state.
- The existing repository still performs all issuance/PKCE validation. Only after
  it rejects issuance does the web handler distinguish the pinned method error
  and route it (or invalid scope) using the selected mode. Missing/syntax PKCE
  guards still reject locally. No repository, grants, schema or grant-type changes.
- Form-post returns no-store HTML with the exact validated callback action, a
  static SHA-256-authorized auto-submit script, manual fallback and a response-local
  callback-origin CSP; common HTML CSP stays unchanged. Set no-referrer for this
  response, retaining common anti-framing/sniffing headers.
- Metadata now advertises the three implemented modes with the documented
  HTTP(S)-only form-post boundary; native OOB success remains local code display.

Independent architecture/DRY review found no blocking correctness, type/lifetime,
call-site or concrete Clippy issues by inspection. It confirmed factoring is
proportionate and no supporting module/schema changes are needed. One nonblocking
follow-up: combined malformed challenge plus wrong/missing method is classified
as a method error, while malformed challenge plus S256 is a local invalid request;
both remain rejected before issuance. Do not duplicate repository validation or
change the frozen tests merely to pin that diagnostic precedence here.

Independent security review found no blocking issue or practical bypass. It
confirmed validation/mutation ordering, unchanged session/CSRF/repository PKCE
checks, escaped non-executable state, exact static-script CSP hashing and
response-local callback CSP. No production changes were recommended by review.
Both reviews were source-only: HTML/CSP assertions model form encoding but do not
execute browser submission. Execution remains parent-owned; no local workloads,
NAS/SSH activity, fetches or commits performed here.

## Parent-verified focused GREEN

Parent applied the web implementation and reported:
- NAS formatting completed.
- Ordinary `metadata_advertises_query_fragment_and_form_post`: **1 passed**.
  Log: `/srv/workspaces/rustodon-audit-green/logs/oauth-metadata-green.log`.
- Real ignored schema selector
  `supported_response_modes_preserve_authorization_security`: **1 passed** in
  **14.48 seconds**.
  Log: `/srv/workspaces/rustodon-audit-green/logs/oauth-modes-green.log`.

These are parent-reported focused results, not independently executed here.
Combined Clippy, schema, ordinary and required differential gates are underway;
no full-gate or pinned-differential pass is claimed. The issue remains open
pending those remaining acceptance checks.

## Completion

NAS metadata and real response-mode fixture tests passed after their recorded
reds. The full schema aggregate, default/all-feature debug/release tests and
strict all-target/all-feature Clippy passed. Pinned `oauth_bearer_authentication`
now passes (`differential-required-remaining.log`); its later unrelated preflight
failure was separately tracked and corrected. Evidence also includes
`schema-remaining.log`, `clippy-corrected.log`, and `{default,feature,release}-remaining.log`.
Separate security, architecture and final integrated reviews found no blockers.
No schema/grant changes, deployment, or weaker PKCE/CSP policy were introduced.
Optional grant-snapshot/error-precedence test strengthening remains out of scope.
