# Style the native applications

Visualizer, Tools, Launcher and the retained desktop application share presentation
through `studio-ui-foundation`. Keep application state and ROS work outside this
layer. Initialize `theme::configure` when creating an application; it also installs
the shared fonts. System, Light and Dark appearance choices continue to use egui's
native theme preference.

## Use semantic colors

Use `theme::palette(ui.visuals())` rather than hardcoded application colors. The
palette has a surface, subtle surface, raised surface, ink, muted ink, border,
accent, text-on-accent, selection, warning, warning surface, success and danger.
A separate control-border token provides stronger boundaries than quiet panel
dividers.
Light appearance uses cool white surfaces and cobalt actions; dark appearance
uses dark neutral surfaces and lighter blue actions. Each theme has its own
text-on-accent color. Selected rows use a tinted surface and accent text.

Use `primary(visuals, label)`, `disconnect(visuals, label)`, `quiet(label)`,
`selectable(visuals, text, selected)`, `view_frame(ui, selected)` and
`badge(ui, label, color)` for their corresponding roles. The default egui button
is the outlined secondary action. Use explicit text for status; color alone must
not communicate success, warning or failure. Preserve measurement and display
colors, including image pixels, map values, plot series and spatial axes.

Controls use 34 logical points for standard actions, 6-point corners, 1-point
borders, 2-point focus strokes, and spacing in the 4/8/12/16/24/32-point scale.
Menus and intentionally compact controls use `COMPACT_HEIGHT` (28 points).
Use `input` and explicitly colored `hint` text for single-line fields, and the
`slider` adapter to keep slider handles independent of button height. Keep fixed
connection action slots stable across state and label changes. Give long values
wrapping, truncation with full-value access, or local scrolling rather than
allowing them to displace neighboring actions.

## Use shared typography

Body text is 14 points, button text 13, metadata and structured data 12, section
headings 18 and page headings 22. `TextStyle::Heading` and the named
`theme::PAGE_HEADING` text style use a semibold family. Use `theme::strong` for
emphasized body labels. Structured data uses the monospace family.

The font resolver reads installed font files once during application creation.
It tries platform preferences (San Francisco/Helvetica Neue and SF Mono/Menlo on
macOS; Segoe UI and Consolas on Windows; Noto/DejaVu/Liberation on Linux), checks
that egui can parse the face, and retains the existing bundled Latin, Noto Sans
SC and Noto Emoji fallbacks. Missing or unreadable system fonts do not prevent
startup. System fonts are read on the user's machine and are not copied into
distributions. Bundled font notices remain in the application.

`fonts::definitions()` clones cached immutable font data. Native UI and spatial
text workers must use this same configuration; do not scan the filesystem or
construct a different font stack in an active frame. Utility icons use consistent
outline strokes. Regenerate Visualizer's Rust icon paths and reviewable SVGs
together with `node scripts/generate-visualizer-icons.mjs`.

## Verify changes

Run focused shared-theme/font and affected-application tests, builds, Clippy and
format checks through locked project tooling. Check text contrast, Unicode
fallback, selected and disabled states, focus, and stable action geometry.
Inspect the compiled applications with synthetic fixtures in both themes at
default and minimum supported sizes, including long values, empty/waiting/error
states, recovery and shutdown. Headless rendering is not native or assistive
technology acceptance. Follow [the testing guide](testing.md) for evidence and
process cleanup.
