# Make public documentation environment-neutral

## Summary

Remove private build-host and workspace details from the public project, make the README a compact project entry point, and move durable technical guidance into focused documents under `docs/`.

## Requirements

- Remove references to owner-specific build hosts, workspace paths, retained machine-local logs, and host authorization policy from the current tracked tree.
- Remove or generalize helper tooling that exists only for one private execution environment.
- Keep test evidence accurate while describing it by gate and result rather than by private host or filesystem location.
- Shorten the README to project status, scope, quick-start development commands, documentation links, repository layout, and license status.
- Add focused project-neutral documentation for architecture/capabilities, testing, and operations as needed.
- Preserve fixture safety, isolation, immutable-source, and no-production-run protections in generic form.

## Acceptance Criteria

- The current tracked tree contains no references to the removed private host names, workspace roots, or host-specific runbook.
- README is substantially shorter and links to the detailed documents.
- All relative Markdown links resolve.
- Offline tooling tests and source checks affected by generalized or removed helper scripts pass in the authorized execution environment.
- An independent correctness/DRY/readability review approves the final changes.

## Notes

- Historical Git commits are not rewritten; this cleanup applies to the current public tree.
- Do not weaken production SSRF, TLS, fixture ownership, cleanup, or test-only feature boundaries.
