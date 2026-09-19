# Support authenticated profile read

## Summary

Bounded prerequisite for [hashtag browser acceptance](accept-local-hashtag-controls-browser-differential.md), related to [frontend settings API support](support-frontend-web-settings-api.md): the profile editor needs authenticated GET `/api/v1/profile` before featured-tag list/removal can render.

## Requirements

- Match pinned Mastodon 4.6.5 profile read serialization and owner-only functional-user authentication; include trailing-slash alias.
- Reuse account and current-user projections; no profile writes, general settings expansion, schema, jobs, or grants.
- Stop if the complete response requires new private extra-information access.

## Acceptance Criteria

- Pure projection and pinned-source tests cover complete response fields.
- Restricted-role HTTP tests cover owner identity, scopes, anonymous/application credentials and disabled users on a disposable PostgreSQL 14 fixture (NAS immutable 7203 tools image), with resource/time bounds.
- Browser acceptance, deployment, and parent review remain separate gates.

## Notes

Implementation and verification pending; keep open. Existing sibling-owned browser harness and documentation are excluded.

## Implementation / verification handoff

- Implemented GET `/api/v1/profile` and its trailing-slash alias only. The route
  uses existing `VERIFY_CREDENTIALS` scopes (`profile`, `read`, `read:accounts`),
  functional-user checks, private caching, and the authenticated owner's account
  and featured-tag projections. Query parameters cannot select a different owner.
- Complete pinned read shape: raw note/fields, formatted note/fields, nullable
  avatar/header URLs, descriptions, profile flags, attribution domains and
  featured tags. Existing credential/account serialization supplies the values;
  no new private root, SQL, grants, schema, jobs, or profile writes were added.
- Inspected exact clean Mastodon revision
  `1440d55b139e39ec722c2a3db7f60b66cd889048`, including profile controller,
  serializer, API base authentication, account-edit screens, API client and reducer.
  Ordinary API OAuth remains required: a cookie alone does not replace its bearer.
- Pure-projection test first failed because `profile` did not exist, then passed.
  HTTP regression was also checked against baseline routing and failed 404 versus
  required 401; final implementation passes. The HTTP test was written after the
  handler, so that portion was regression red/green rather than strict test-first.
- Executed sequentially in disposable NAS PostgreSQL 14.23 and immutable tools
  image `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`:
  - all-feature library: **356 passed, 22 ignored**;
  - REST serializers: **19 passed**;
  - verified pinned-source contracts: **10 passed**;
  - restricted-runtime profile HTTP: **1 passed** on final tree (both aliases,
    all three accepted scopes, write-only scope rejection, anonymous/invalid
    token/session-cookie rejection, application token, disabled/unapproved/
    unconfirmed users, distinct owners, complete fields, featured-tag equality,
    parameter isolation, private cache policy);
  - strict all-feature library Clippy, focused formatting and diff checks passed.
- Test containers were bounded to 4 CPUs / 6 GiB / 512 processes / 870 seconds
  (outer timeout 900 seconds); PG to 2 CPUs / 1 GiB / 128 processes / 3600 seconds.
  Task-owned database/network only; no host-published ports or production access.
  Existing runtime/writer fixture grants were used unchanged; HTTP state itself
  had no writer repository. Final fixture restored after intermediate setup
  failures; those failures are not counted as passing evidence.
- Native macOS test execution is unsupported by existing Linux-only Paperclip
  imports; executable evidence above is from the bounded Linux tools image.
- Evidence retained under ignored `target/profile-read-evidence/` and remote
  `/srv/workspaces/rustodon-profile-read-alice/evidence/`. Task PG and network
  removed after testing. No browser, deploy, push, commit, or staging performed.
- Open for parent review and browser continuation. Independent child review was
  unavailable (agent nesting limit); no independent review is claimed. Sibling
  documentation/harness left untouched except the requested new issue index entry.
