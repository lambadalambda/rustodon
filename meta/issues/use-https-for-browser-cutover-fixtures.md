# Use HTTPS for browser cutover fixtures

## Summary

Real frontend settings PUT arrives promptly with the correct object and Home
flags, but receives 422. The fixture navigates to HTTP while Rustodon uses an
HTTPS origin and a Secure `__Host-csrf_token` cookie. Curl login explicitly sends
forwarded HTTPS, so it does not prove the browser can retain/send that cookie.
The exact 422 branch is being classified without logging tokens.

## Requirements

- Align the browser fixture transport with its configured HTTPS origin using a
  task-owned loopback TLS endpoint and ephemeral certificate trust.
- Preserve production CSRF/cookie policy; do not bypass token checks or turn off
  certificate verification globally. Curl must validate the fixture CA/hostname.
- Keep fixture resource ownership, bounded lifecycle, read-only source and actual
  delayed-save predicates intact. No live service, global trust-store or host
  network configuration changes.
- TDD helper/guard coverage and independent review, all execution on an isolated worker.

## Acceptance Criteria

- Fixture TLS lifecycle/transport tests pass and real browser saves retain valid
  secure cookies; no secret/certificate-key leakage or leftover task processes.

## Evidence

- A historical external run recorded the first PUT 422 in 14 ms; expected payload
  flags matched, with no `saved` and no preceding settings requests. This was not
  a timing failure.
- `tools/mastodon-fixture` passes HTTP base_url to browser; configured origin HTTPS.

## Verified implementation

- A historical external run confirmed `errorClass=csrf`, `browserHttps=false`,
  header/meta token present, and valid first PUT 422 completed in 15 ms. Secure-
  cookie transport mismatch was the failure, not debounce timing or payload shape.
- A task-owned standard-library TLS relay transparently streams to the fixed
  loopback backend. It uses one-day exact-domain CA/leaf material, a mode 700
  directory and mode 600 key, bounded readiness, and bounded shutdown; keys are
  discarded even when other fixture artifacts are retained.
- Curl disables ambient curl configuration and validates the CA/hostname;
  Chromium uses only the exact per-run leaf SPKI. No global trust-store changes,
  production cookie/CSRF changes, insecure retry, or shared peer-proxy widening.
- A historical external isolated-worker RED recorded 8 failing tests; the GREEN
  recorded 10 passing tests, including wrong trust/hostname, duplicate
  cookies, >2 MiB upload/download, duplex traffic and active/stalled cleanup.
  A historical external run failed before the adapter; a later run passed
  actual extracted JS and shell adapter tests afterward.
- A historical external run recorded the actual browser smoke passing authenticated settings
  leading/trailing PUT, reload persistence, API audits and logout. The enclosing
  cutover later failed Mastodon Puma readiness; that separate gate was pending at
  this intermediate checkpoint (see final acceptance below).
- Separate component reviews and merged security/correctness/compactness review
  approved. Tests are permanently discovered by `tools/check-harnesses`.

Final acceptance: a historical external run passed the complete authenticated HTTPS
browser-plus-cutover gate. A separate historical external run passed the ordinary
cutover. Parent inspection after prior runs found no task TLS directories or
rootful fixture containers. Certificate cleanup also passes the permanent tests.
