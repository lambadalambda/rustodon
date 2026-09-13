# Use HTTPS for browser cutover fixtures

## Summary

Real frontend settings PUT arrives promptly with the correct object and Home
flags, but receives422. The fixture navigates to HTTP while Rustodon uses an
HTTPS origin and a Secure `__Host-csrf_token` cookie. Curl login explicitly sends
forwarded HTTPS, so it does not prove the browser can retain/send that cookie.
The exact422 branch is being classified without logging tokens.

## Requirements

- Align the browser fixture transport with its configured HTTPS origin using a
  task-owned loopback TLS endpoint and ephemeral certificate trust.
- Preserve production CSRF/cookie policy; do not bypass token checks or turn off
  certificate verification globally. Curl must validate the fixture CA/hostname.
- Keep fixture resource ownership, bounded lifecycle, read-only source and actual
  delayed-save predicates intact. No live service, global trust-store or host
  network configuration changes.
- TDD helper/guard coverage and independent review, all execution on NAS.

## Acceptance Criteria

- Fixture TLS lifecycle/transport tests pass and real browser saves retain valid
  secure cookies; no secret/certificate-key leakage or leftover task processes.

## Evidence

- `browser-settings-diagnostics.log`: first PUT422 in14ms, expected payload flags
  match, no `saved`, no preceding settings requests. Not a timing failure.
- `tools/mastodon-fixture` passes HTTP base_url to browser; configured origin HTTPS.

## Verified implementation

- `browser-classifier-http.log` confirms `errorClass=csrf`, `browserHttps=false`,
  header/meta token present, and valid first PUT422 completed in15ms. Secure-cookie
  transport mismatch was the failure, not debounce timing or payload shape.
- Task-owned stdlib TLS relay transparently streams to the fixed loopback backend.
  One-day exact-domain CA/leaf, mode700 directory/key600, bounded readiness and
  shutdown; keys are discarded even when other fixture artifacts are retained.
- Curl disables ambient curl configuration and validates the CA/hostname;
  Chromium uses only the exact per-run leaf SPKI. No global trust-store changes,
  production cookie/CSRF changes, insecure retry, or shared peer-proxy widening.
- NAS `browser-tls-red.log`:8 tests failed before implementation;
  `browser-tls-green.log`:10 tests pass, including wrong trust/hostname, duplicate
  cookies, >2MiB upload/download, duplex traffic and active/stalled cleanup.
  `browser-trust-red.log` fails before adapter; `browser-trust-green.log` passes
  actual extracted JS and shell adapter tests afterward.
- `browser-https-first.log`: actual browser smoke passes authenticated settings
  leading/trailing PUT, reload persistence, API audits and logout. The enclosing
  cutover later failed Mastodon Puma readiness; that separate gate was pending at
  this intermediate checkpoint (see final acceptance below).
- Separate component reviews and merged security/correctness/compactness review
  approved. Tests are permanently discovered by `tools/check-harnesses`.

Final acceptance: `final19-browser.log` passes the complete authenticated HTTPS
browser-plus-cutover gate. `final19-cutover.log` independently passes the ordinary
cutover. Parent inspection after prior runs found no task TLS directories or
rootful fixture containers. Certificate cleanup also passes the permanent tests.
