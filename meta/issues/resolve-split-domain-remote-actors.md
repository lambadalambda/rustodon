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
