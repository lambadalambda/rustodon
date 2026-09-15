# Style Rustodon-owned HTML surfaces

## Summary

Give Rustodon's server-rendered authentication, OAuth, recovery, confirmation, and account-settings pages a cohesive responsive presentation aligned with the pinned Mastodon frontend instead of unstyled browser defaults.

## Requirements

- Use a shared document shell and stylesheet across every user-visible Rustodon-owned HTML page.
- Match Mastodon's typography, spacing, colors, panels, controls, navigation, and destructive-action treatment without copying deployment-specific content into the repository.
- Preserve existing routes, form names/actions, CSRF behavior, escaping, accessibility semantics, and OAuth response-mode security.
- Support narrow and wide viewports, keyboard focus, reduced motion, and light/dark color schemes without requiring JavaScript.

## Acceptance Criteria

- Unit tests fail before and pass after the shared shell/style integration for authentication, OAuth, recovery/confirmation, and settings documents.
- Browser checks show usable Mastodon-like layouts at desktop and mobile widths, with no horizontal form overflow and clearly visible focus, alert, primary, secondary, and danger states.
- Formatting, relevant tests, and strict lint pass.
- An independent correctness, security, accessibility, and maintainability review approves the change before commit.

## Notes

- This is product UI work. The incidental `rustodon.social` deployment remains outside repository documentation and issue records.
