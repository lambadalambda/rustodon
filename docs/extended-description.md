# Instance extended description

`GET /api/v1/instance/extended_description` serves the administrator's
`site_extended_description` setting. The trailing-slash alias, automatic HEAD,
and shared API OPTIONS/CORS handling follow existing instance surfaces.

## Source and verification boundary

The implementation was derived from the exact cached Mastodon 4.6.5 image,
digest
`696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`,
without network access. No external extraction artifact is required in a clean
checkout. The committed serializer expectations, fixture metadata, and pinned
revision `1440d55b139e39ec722c2a3db7f60b66cd889048` are the durable public contract;
none of this modifies or substitutes for the separate pinned-source oracle.

The pinned serializer spec directly establishes:

- `Hello world` renders as `<p>Hello world</p>\n`.
- `2024-11-28T16:20:00` serializes as `2024-11-28T16:20:00+00:00`.
- Missing/blank description yields empty content and a null timestamp.

Additional compatibility cases in `src/web/extended_description_tests.rs` cover
paragraph/heading separation, emphasis, links, inline code, simple lists,
blockquotes, soft/hard breaks, horizontal rules, and trusted HTML. These are
explicit expected-byte regressions checked against the pinned engine in an
isolated fixture: **11/11 passed**. All five Rust
serializer units and both normal/limited-mode HTTP cases also passed. The browser
lane passes its anonymous startup API audit and authenticated delayed-save/reload
persistence checks. This corpus is not complete dialect parity.

## Storage and errors

A single read-only query loads only the named setting's value and `updated_at`.
There are no setting writes or additional runtime grants. `RawYamlText::parse`
decodes safe YAML, including Rails' quoted, literal, and folded string forms.
It does not use the older scalar-stripping instance helper. Nonblank strings
are rendered without trimming their Markdown indentation. NULL, YAML null/false,
and whitespace-only strings produce `{"updated_at":null,"content":""}`.
Malformed YAML, multiple documents, and unsupported non-string configuration
return HTTP 500, rather than silently replacing configured content with an empty
page. Nonblank content with a nullable database timestamp retains its content;
otherwise timestamps use UTC, whole-second ISO8601 with the explicit `+00:00`
offset, not the ordinary REST millisecond timestamp format.

## Markdown scope and known differences

`pulldown-cmark` uses the deliberate Cargo requirement `0.13.0` (caret semantics:
`>=0.13.0, <0.14.0`), with default features disabled and only `html` enabled.
This is a maintained Rust library parser/renderer, with no CLI, FFI, or native
Redcarpet build dependency. The resolved lockfile pins `pulldown-cmark` 0.13.4 and its escape helper
0.11.0; no existing dependency versions changed.

This is **not full Redcarpet dialect parity**. The parser is CommonMark; no
optional tables, strikethrough, footnotes, task lists, or smart punctuation are
enabled. An event-level adapter supplies Redcarpet-style blank lines between
sibling blocks and HTML-style `<br>`/`<hr>` output without rewriting raw HTML or
code strings. Configured Markdown is actually rendered, not downgraded to the
post/profile plain-text formatter.

Known dialect/serialization differences to retain visibly until separately
scoped and verified:

- CommonMark fenced code blocks are built in; default Redcarpet requires its
  `fenced_code_blocks` extension to recognize fences. This adapter does not
  pretend that disabling optional CommonMark extensions disables fences.
- CommonMark emphasis, HTML-block, list interruption/nesting and reference-link
  parsing rules can differ from Redcarpet's defaults outside the tested corpus.
- Image rendering retains pulldown-cmark's XHTML-style void-tag closing syntax;
  the `<br>`/`<hr>` compatibility adapter is not a general HTML serialization
  rewrite. Complex image/list/code cases are not certified by the small corpus.
- Safe YAML parsing is not arbitrary Ruby deserialization. Unexpected tagged or
  non-string objects are rejected rather than executed or coerced into markup.
  One deferred non-string edge case is empty YAML arrays/maps: Rails treats them
  as blank; this endpoint currently returns 500. They are outside the supported
  administrator string-setting corpus.

Raw HTML is intentionally retained because the source calls
`Redcarpet::Render::HTML` without sanitizing or filtering options. This helper is
**only for privileged administrator-authored settings**, not user posts, remote
content, or untrusted profiles. No endpoint is added for writing those settings.

## Authentication and caching

The existing `instance_runtime.limited_federation` flag is the only mode switch;
no new environment/configuration feature is introduced.

- Normal mode ignores bearer/session identity, including stale bearer tokens,
  as the pinned controller's `current_user` override does.
- Limited mode requires a functional user without inventing an OAuth read scope.
  A valid token resource owner takes precedence over a browser session. When no
  bearer resource owner exists, the existing browser-session lookup and its
  associated token lifecycle checks supply the fallback. Existing user-state
  errors reject unconfirmed, pending, disabled, moved, suspended and missing-2FA
  users; unauthenticated requests receive the pinned HTTP 401 user-required JSON.
- Successful responses use existing public instance cache/CORS policy, including
  in limited mode: upstream explicitly calls `cache_even_if_authenticated!` and
  its instance base clears identity variation. Errors remain private/no-store.
  OPTIONS preflight remains unauthenticated. Existing v1/v2/rules behavior is
  deliberately untouched by this endpoint-specific change.

The guarded differential tests exercise the real router. Fixture setup uses the
existing marked owner connection only, restores the complete original setting,
and must run sequentially on a disposable fixture—not a live or shared database.
