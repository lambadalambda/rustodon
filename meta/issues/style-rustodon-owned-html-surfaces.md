# Style Rustodon-owned HTML surfaces

## Summary

Give Rustodon's server-rendered authentication, OAuth, recovery, confirmation, and account-settings pages a cohesive responsive presentation aligned with the pinned Mastodon frontend instead of unstyled browser defaults.

## Requirements

- Use a shared document shell and stylesheet across every user-facing Rustodon-owned HTML page except the OAuth `form_post` auto-submit transport, whose no-script fallback remains deliberately minimal to preserve its exact CSP and immediate callback behavior.
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

## Implementation

- Added one content-hashed, embedded Rustodon stylesheet and a shared compact/settings document shell for sign-in, password recovery, confirmation errors, OAuth consent/OOB results, and all account-settings pages.
- Added responsive Mastodon-like light/dark palettes, branded layout, settings navigation, polished controls, alerts, keyboard focus, reduced-motion handling, wrapped codes, and explicit secondary/danger actions. Form actions, names, CSRF fields, escaping, response headers, and the specialized OAuth `form_post` transport remain unchanged.
- TDD observed the existing unstyled documents fail the new shell and palette assertions. Six focused HTML/CSS/OAuth tests then passed; formatting, `git diff --check`, and strict all-feature library/test Clippy passed.
- Agent-browser checks passed at 1280×900 and 390×844 in light mode and at 390×844 in dark mode. Mobile sign-in/settings had no horizontal overflow, inputs remained inside 358px cards, settings collapsed to one column, and keyboard focus rendered a solid 3px accent outline.
- Independent correctness, security, accessibility, responsive-design, cache/CSP, and maintainability review approved the final implementation with no blockers.
