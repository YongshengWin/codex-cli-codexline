# Codexline Design System

## Product context

Codexline is a cross-platform terminal companion for the official Codex CLI.
It adds an always-visible, responsive status line without modifying Codex. The
product consists of the status line itself, an interactive terminal-native
configuration center, diagnostic commands, and an open-source GitHub presence.

Primary users are developers who spend long sessions in Codex and need to see
model, context pressure, active work, tools, agents, Git state, permissions, and
elapsed time without interrupting their flow.

## Key experiences

1. The status line is informative at a glance and never dominates the terminal.
2. `codexline config` opens a guided terminal UI with live preview.
3. The first step chooses whether users keep typing `codex` through a reversible
   shim or explicitly launch `codexline` without installing a shim.
4. Users can start from Full, Focus, or Minimal presets and then customize.
5. The same configuration previews wide, narrow, ASCII, and monochrome modes.
6. Capability and fallback state are visible without exposing implementation
   noise during normal use.

## Visual direction

Use a high-contrast, typography-first developer-tool aesthetic inspired by
Swiss minimalism, adapted for terminal constraints. The interface should feel
precise, calm, engineered, and fast rather than playful or decorative.

- Prefer strong hierarchy, alignment, whitespace, thin rules, and compact data.
- Avoid glassmorphism, ornamental gradients, excessive glow, fake 3D depth, and
  generic dashboard cards.
- The configuration UI must look implementable in a real terminal, not like a
  web settings page placed inside a terminal frame.
- Use symbols that have clear ASCII fallbacks. Do not require Nerd Font.

## Color tokens

Dark is the primary terminal presentation:

- Canvas: `#0B0D0E`
- Surface: `#111416`
- Elevated selection: `#1A1F21`
- Primary text: `#F1F5F2`
- Secondary text: `#98A39D`
- Dim text: `#606A65`
- Rule/border: `#29302D`
- Accent: `#6EE7A8`
- Accent strong: `#34D17B`
- Information: `#67D8EF`
- Warning: `#F3C969`
- Danger: `#F37B83`
- Focus ring: `#A7F3D0`

Light preview mode:

- Canvas: `#F3F4F1`
- Surface: `#FFFFFF`
- Primary text: `#111412`
- Secondary text: `#59635E`
- Rule/border: `#CED4D0`
- Accent: `#087A45`

Color is supportive, never the only carrier of meaning. Every state also has a
label, symbol, or pattern.

## Typography

- Terminal UI and previews: the user's terminal monospace font.
- README diagrams and SVG assets: `ui-monospace`, `SFMono-Regular`, `Cascadia
  Code`, `JetBrains Mono`, `Menlo`, `Consolas`, monospace.
- Marketing headings may use a system geometric sans stack, but no external web
  font is required.
- Use concise sentence case. Avoid all-caps paragraphs.

## Spacing and geometry

- Base spacing unit: 1 terminal cell horizontally and 1 row vertically.
- Configuration center target: minimum 80x24, comfortable at 110x32.
- One-cell padding inside groups; one blank row between major regions.
- Terminal panels use square corners and single-line box drawing in Unicode
  mode, with `+|-` fallbacks in ASCII mode.
- Selection is shown with a left marker, inverted row, or accent foreground;
  never rely on a subtle background difference alone.

## Configuration center layout

Use a three-region terminal-native layout:

1. Left rail (18-22 columns): Launch, Presets, Layout, Modules, Theme,
   Compatibility, Advanced.
2. Main editor (flexible): controls for the active section, keyboard-operable
   toggles, ordering, priority, compact representation, and thresholds.
3. Live preview (32-46 columns or full-width lower pane on narrow terminals):
   wide, narrow, ASCII, and degraded-data preview modes.

Persistent bottom help row shows keys such as `↑↓ move`, `space toggle`,
`enter edit`, `p preview mode`, `r reset`, `s save`, `q cancel`.

The guided flow begins with a Launch screen. The recommended selection keeps
the `codex` command through a reversible user-level PATH shim and clearly states
that the official binary is not overwritten. The alternative uses `codexline`
explicitly and installs no shim. Show resolved shim/binary paths, PATH health,
`--no-companion` bypass, and a dry-run summary before continuing.

The top row shows `CODEXLINE CONFIG`, config path, validation state, and whether
changes are saved. Do not use a web-style top navigation bar.

## Presets

- **Full**: model, reasoning, turn/tool, context bar, agents, plan progress, Git,
  permissions, elapsed time.
- **Focus**: turn/tool, context percentage, agents, Git.
- **Minimal**: model, context percentage, Git branch.

Presets are starting points. Changing individual options creates a `Custom`
state without losing the original preset.

## Status-line grammar

- Segments are separated by a dim `│` in Unicode mode and `|` in ASCII mode.
- Active work uses a motion-safe spinner only when animations are enabled;
  otherwise use a stable `RUN` label.
- Context uses both bar and percentage when space allows, percentage only when
  narrow.
- Missing fields disappear; unknown values never render as zero.
- Git dirty state uses `*` in every glyph mode.

Wide example:

` Codex gpt-5.6 high │ ⟳ exec 8s │ ctx ▓▓▓░░ 42% │ ↑2 agents │ feat/hud * `

Narrow example:

` ⟳ exec 8s │ ctx 42% │ main* `

## Motion

- Redraws are event-driven and coalesced.
- Spinner maximum is 8 FPS; respect reduced motion and monochrome modes.
- Configuration transitions are immediate. Do not animate panel movement.
- Save success may show a short two-state check indicator, never a blocking
  toast.

## README visual language

- Begin with product name, one-sentence value proposition, terminal preview,
  and install/status badges.
- Use a total-to-detail narrative: value, preview, install, what it shows,
  configuration, how it works, compatibility, performance, security, roadmap,
  contributing.
- Favor diagrams, compact tables, terminal captures, and short paragraphs.
- Do not claim features as shipped before implementation. Clearly mark planned
  commands and screenshots as design previews.
- README assets use the same terminal palette and avoid rasterized text when an
  SVG can remain sharp and accessible.

## Accessibility

- Maintain high contrast for primary and secondary text.
- All color-coded health states include text or symbols.
- Provide ASCII, no-color, and reduced-motion modes.
- Keyboard-only configuration is mandatory.
- Preview screen-reader-friendly linear output alongside the visual HUD.

Use ONLY the colors, typography, spacing, and terminal-native component styles
defined here. Do not introduce additional fonts, decorative palettes, rounded
web cards, gradients, or visual styles.
