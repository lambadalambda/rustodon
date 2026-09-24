# Resolve split-domain remote actors

## Summary

Remote actors on split-domain servers (`WEB_DOMAIN` ≠ `LOCAL_DOMAIN`), for example
actor `https://mastodon.bsd.cafe/users/jae` with WebFinger subject
`acct:jae@bsd.cafe`, cannot be resolved by URI. `parse_webfinger_document`
(`src/remote.rs`) requires the subject domain to equal the actor host, so
resolution fails with `IdentityMismatch`. On rustodon.social this dead-lettered
16 reply-parent resolutions (bsd.cafe, maid.zone, feministwiki.org, gnuweeb.org,
ree.social, linksjugend-solid.de) between 2026-09-15 and 2026-09-22. The same
check runs for signature key owners, so inbound activity can be affected too.

## Requirements

- Match Mastodon: WebFinger the actor host; if the subject names another
  domain, WebFinger that subject and require its `self` link to equal the actor
  URI. Store the account under the subject domain.
- Decide how existing rows (created with the actor host as domain, or imported
  from Mastodon with the subject domain) converge without duplicates.
- Domain blocks and mentions must use the account domain consistently.

## Acceptance Criteria

- Unit tests for the two-step WebFinger with a mocked split-domain server.
- A restored-fixture regression proves a reply to a split-domain parent threads.

## Notes

- Scope checkpoint (AGENTS.md): this changes stored remote identity. Needs a
  decision before implementation.

## Implementation 2026-09-24

- `confirm_webfinger` (src/remote.rs) follows Mastodon's `check_webfinger!`: ask the
  handle's domain; if the subject names another account, ask that domain once and
  require it to name itself and this exact actor. `RemoteActor.domain` carries the
  confirmed account domain; the plain actor parser still refuses a foreign domain.
- Handle lookups (`@jae@bsd.cafe`) with a cross-origin self link resolve the actor
  by URI and require the confirmed handle to equal the request.
- Persistence stores `actor.domain`; domain policy is checked for both the account
  domain and the actor host (a blocked host cannot hide behind an allowed domain).
  `RemoteKeyResolution.domain` and ingress job domains stay host-based; the known-key
  ingress check compares the actor URI, not `accounts.domain`.
- Tests: offline confirmation tests (redirect, no second redirect, self-link and
  subject mismatches), parser guard, and `worker-test split_domain` (red without the
  host check, green with it).

## Remaining limits (fail closed)

- `acct:` signature keyIds whose domain differs from the actor host are refused.
- An FEP-2c59 actor whose confirmed username differs from `preferredUsername` is
  refused (Mastodon accepts it).
- Limited federation allow-lists match the actor host exactly.

## Done 2026-09-24

Regression check: the full worker lane gives 93 passed / 29 failed both with and
without this change; the failures are old ones (see
[repair-failing-worker-lane-tests](repair-failing-worker-lane-tests.md)).
An independent security review found three blockers (host policy, key domain,
known-key ingress), all fixed before commit.
