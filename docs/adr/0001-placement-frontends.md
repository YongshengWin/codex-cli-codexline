# ADR 0001: Separate placement frontends from status capabilities

Status: accepted

## Context

The initial prototype reserves the terminal's final row. This is robust, but it
can sit far away from Codex's composer and visually duplicates the native
footer. The product requirement is a more attractive and substantially richer
status experience located near the composer, while preserving a fallback that
survives Codex and terminal changes.

Codex currently documents an ordered list of built-in `tui.status_line` item
identifiers. It does not document a command-backed third-party status provider.
Hooks and app-server events provide state, not stable TUI coordinates.

## Decision

Keep state collection, layout, theming, and modules independent of placement.
Provide these frontends:

- `attached`: compositor near the Codex composer; primary product experience.
- `bottom`: reserved final terminal row; compatibility fallback.
- `auto`: attached when its capability check is trustworthy, otherwise bottom.
- `off`: transparent relay without Codexline rendering.

Attached positioning may use bounded terminal state observation for cursor,
viewport, alternate-screen, and damage tracking. It must not inspect prompts or
responses, infer agent state from screen text, or grow into a Codex TUI fork.
Unknown Codex versions and ambiguous positioning must select `bottom`.

The configuration UI may offer to disable the native footer to prevent visual
duplication. That operation requires a preview, explicit consent, a recoverable
backup, and conservative uninstall restoration.

## Consequences

- Every status module works with every Codexline-owned frontend.
- Attached mode can evolve without putting the relay or fallback at risk.
- The initial bottom renderer remains useful rather than being discarded.
- Exact attached compatibility requires a versioned capability matrix and
  terminal regression fixtures.
- A future upstream custom provider becomes another frontend, not a rewrite.
