<p align="center">
  <img src="assets/hero.svg" alt="Codexline terminal preview" width="100%" />
</p>

<h1 align="center">Codexline</h1>

<p align="center">
  A fast, configurable, cross-platform companion status line for the official Codex CLI.
</p>

<p align="center">
  <code>macOS</code>&nbsp;&nbsp;<code>Linux</code>&nbsp;&nbsp;<code>Windows</code>&nbsp;&nbsp;<code>WSL</code>
</p>

> [!IMPORTANT]
> Codexline is currently an **unreleased M1 prototype**. PTY/ConPTY launch,
> passthrough fallback, a static responsive status row, preview, doctor, and
> launch-mode configuration are implemented. Rich modules and the visual
> configuration screens below remain the approved product direction.

Codexline keeps the state that matters visible—model, context pressure, active
work, agents, Git, permissions, and elapsed time—without modifying Codex or
requiring tmux.

Its quality bar is deliberately higher than the built-in footer: stronger
visual hierarchy, richer live state, responsive layouts, polished themes, and
clear degradation when a data source is unavailable. Recoloring the native
fields is not the product.

## Why Codexline?

| Stay oriented | Stay responsive | Stay compatible |
| --- | --- | --- |
| See what Codex is doing without interrupting the session. | Event-driven rendering keeps the terminal relay path fast. | Codexline wraps the official process and falls back safely when an integration is unavailable. |

## What you will see

```text
 Codex gpt-5.6 high │ ⟳ exec 8s │ ctx ▓▓▓░░ 42% │ ↑2 agents │ feat/pty *
```

The layout responds to available width:

```text
# Wide
Codex gpt-5.6 high │ ⟳ exec 8s │ ctx ▓▓▓░░ 42% │ ↑2 agents │ feat/pty *

# Narrow
⟳ exec 8s │ ctx 42% │ main*

# ASCII / no special glyphs
gpt-5.6 | RUN 8s | context 42% | main*
```

| Module | Signal |
| --- | --- |
| Model | Active model and reasoning effort |
| Work | Turn state, active tool, and elapsed time |
| Context | Context pressure, tokens, and warning thresholds when available |
| Agents | Active and completed subagents |
| Plan | Current step and task progress |
| Git | Branch, dirty state, staged files, and ahead/behind |
| Safety | Permission mode, sandbox, and degraded integrations |

Unknown data is hidden rather than rendered as a misleading zero.

The primary product direction is an `attached` layout directly below the Codex
composer. A compatibility-first `bottom` dock remains available, and `auto`
falls back to it whenever attached positioning is not trustworthy.

```text
┌─ Codex composer ──────────────────────────────────────────────┐
│ Implement {feature}                                          │
└──────────────────────────────────────────────────────────────┘
 Codex gpt-5.6 high │ ⟳ exec 8s │ ctx 42% │ ↑2 agents │ main *
```

## Try the prototype

The project currently builds with Rust 1.85 or newer:

```bash
cargo build
cargo run -- preview --width 100
cargo run -- doctor
cargo run -- run -- --help
```

Use an explicit binary when Codex is not discoverable on `PATH`:

```bash
CODEXLINE_CODEX_BIN=/absolute/path/to/codex cargo run -- run
```

Non-TTY output, `TERM=dumb`, CI, `codex exec`, small terminals, and
`--no-companion` automatically run the official Codex without an overlay.
Codexline is not yet published and does not install a shim in this milestone.

## Configure it visually

`codexline config` opens a fixed-viewport, keyboard-first terminal editor. It
uses one alternate-screen view instead of scrolling prompts: `←/→` changes
sections, `↑/↓` moves, `Space` or `Enter` selects, and `S` saves. Launch mode,
presets, modules, appearance, data sources, review, and a live HUD preview all
share one staged snapshot. Options use the full upper viewport while the live
HUD preview stays anchored at full width below them, matching the final HUD's
available width. Set `CODEXLINE_CONFIG_LINEAR=1` for the accessible line-by-line
fallback.

The Modules section groups signals into **Core**, **Usage**, **Workspace**,
**Activity**, and **Runtime**. `←/→` switches categories and `↑/↓` moves
through the selected category; `Tab` returns to the main section navigation.
The editor exposes both compact summaries and granular real-data fields,
including context remaining/used/window, individual token counters, separate
5-hour and weekly limits, Git counts and sync state, agent count, thread ID,
project root, and independent hooks/app-server health.

The editor has explicit primary-tab, secondary-tab, and option-list focus.
`↑/↓` moves between those levels, `←/→` changes the currently focused tab row,
`Space` changes an option, and `Enter` validates and saves from anywhere.

### 1. Guided setup

First choose how Codexline should launch, then select **Full**, **Focus**, or
**Minimal**, preview the result, and save. The guided flow targets first-time
setup and terminals as small as `80×24`.

<p align="center">
  <img src="assets/config-wizard.svg" alt="Codexline guided setup wizard" width="100%" />
</p>

```text
Launch ↔ Preset ↔ Modules ↔ Appearance ↔ Data ↔ Review
```

The Launch step offers two explicit modes:

| Mode | Command you type | What Codexline changes |
| --- | --- | --- |
| Keep `codex` command (recommended) | `codex` | Records shim mode for the reversible installer; never overwrites the official binary |
| Use explicit companion command | `codexline` | Records explicit mode; the official command remains directly selected by the shell |

The editor stages every change until `S` or Review → Save. In shim mode,
`codex --no-companion` bypasses an installed overlay and launches the official
binary. All other arguments and the child exit code pass through unchanged.

### 2. Planned advanced controls

The fixed-viewport editor currently covers the complete public v2 schema.
Module reordering, per-width priority, thresholds, scenario simulation, and an
inline TOML diff are the next advanced controls.

<p align="center">
  <img src="assets/config-advanced.svg" alt="Codexline advanced configuration editor" width="100%" />
</p>

The editor previews:

- wide and narrow terminal widths;
- Unicode, ASCII, monochrome, and reduced-motion modes;
- normal, warning, and degraded-data scenarios;
- the exact TOML changes before saving.

Implemented commands:

```text
codexline run -- ...   # run official Codex; arguments pass through unchanged
codexline config       # fixed-viewport keyboard editor with live preview
codexline preview      # preview the current static responsive status line
codexline doctor       # inspect discovery, TTY state, backend, and config path
```

The prototype bottom dock supports configurable segment order and visibility in
`~/.config/codexline/config.toml`:

```toml
[display]
theme = "ox96f" # transparent 0x96f neon palette; "inherit" uses terminal colors
glyphs = "unicode"
refresh_hz = 8
rows = 3
segments = [
  "app", "model", "work", "context", "tokens", "rate-limits",
  "git", "worktree", "tools", "agents",
  "plan", "compactions", "safety", "elapsed", "cwd", "status"
]
separator = " │ "
```

Available segments are `app`, `model`, `work`, `context`, `tokens`,
`rate-limits`, `git`, `worktree`,
`tools`, `agents`, `plan`, `compactions`, `safety`, `elapsed`, `cwd`, and
`status`. Remove a name to hide it or reorder the array to move it. `inherit`
maps semantic colors through the terminal palette. `ox96f` uses the high-contrast
cyan, green, yellow, violet, and red 0x96f palette. Both remain transparent; the
fixed `codex-dark` and `codex-light` themes paint their own backgrounds.
`preview` uses clearly labelled simulated state to demonstrate the responsive
layout.

When subagents are present, Codexline automatically expands a read-only Agent
Inspector below the regular HUD. It shows up to three live agent rows without
capturing normal input. Press the visible `Ctrl+G focus` action, use `↑/↓` to
select, `Enter` to view the agent goal and latest official app-server message,
and `Esc` to go back or close the inspector.

`rate-limits` is populated through a separate, read-only official Codex
app-server process. It shows the available 5-hour/weekly windows, remaining
percentage, explicit `reset 2d 5h` countdown, and reset credits. Disable that process without
affecting Codex or Hooks:

```toml
[sources]
app_server = false
remote_proxy = false
```

The default is `app_server = true` and `remote_proxy = false`: a separate,
read-only app-server sidecar supplies account capacity while the official Codex
TUI remains on its normal transport. A sidecar failure therefore cannot stop the
interactive session.

`remote_proxy = true` is an explicit experimental option. It routes the TUI
protocol through a loopback WebSocket proxy so `context`, `tokens`, and agent
events can follow the same live thread. Startup and handshake failures fall back
quickly, but a disconnect after the session is established can terminate the
official TUI; the guided wizard calls out that tradeoff. Version 1 configurations
are automatically migrated to version 2 with the proxy disabled. Codexline does
not read private transcripts or scrape the terminal.

With the optional local `codexline-events` plugin trusted, the HUD receives
tool, subagent, plan, approval, and compaction events from official Codex Hooks.
Without it, those unavailable fields remain hidden and Codexline falls back to
model, Git, worktree, safety, elapsed time, and directory probes.

Planned commands and extensions:

```text
codexline setup       # choose launch mode and install integration
codexline themes      # browse built-in themes
codexline uninstall   # remove only files created by Codexline
```

Advanced users will also be able to edit:

```toml
version = 2

[launch]
mode = "shim" # shim | explicit
bypass_flag = "--no-companion"

[display]
theme = "inherit"
glyphs = "unicode"
refresh_hz = 8

[sources]
app_server = true
remote_proxy = false # experimental; enable only when live-thread data is worth the risk

[[layout.left]]
module = "turn"
priority = 100

[[layout.center]]
module = "context"
style = "bar"
priority = 80

[[layout.right]]
module = "git"
priority = 70
```

The current launcher ignores an invalid configuration and starts Codex with
safe defaults. Last-known-good hot reload arrives with M2.

## How it works

Codexline is a thin terminal companion, not another Codex distribution.

```mermaid
flowchart LR
    User["Terminal input"] --> Companion["Codexline"]
    Companion --> PTY["PTY / ConPTY"]
    PTY --> Codex["Official Codex CLI"]
    Codex --> PTY
    PTY --> Companion
    Companion --> Screen["Terminal output + reserved status row"]

    Hooks["Official Hooks"] --> State["Versioned state engine"]
    App["Optional app-server adapter"] --> State
    Local["Git / time / terminal probes"] --> State
    State --> Companion
```

If the real terminal has 40 rows and the Full preset reserves three lanes,
Codexline gives Codex a 37-row child PTY/ConPTY and owns the final three rows:

```text
Real terminal: 120 × 40
├─ Official Codex child terminal: 120 × 37
└─ Codexline HUD lanes:              rows 38–40
```

Codex output is forwarded as bytes. The relay does not wait for Git, JSON,
configuration, or rendering work.
For companion-managed interactive sessions, Codexline adds a process-local
`tui.status_line=[]` override so the native footer does not compete with the HUD.
An explicit user `-c tui.status_line=...` argument still wins, and bypassed or
non-interactive sessions remain untouched.

When shim mode is enabled, process resolution is:

```text
codex (user-level shim) → codexline → PTY/ConPTY → official codex
```

The installer verifies that the shim precedes the official binary in PATH,
records every file it creates, and provides a reversible uninstall. Explicit
mode skips the shim entirely.

### Data sources

1. **Official Hooks** provide stable lifecycle, model, tool, permission, and
   subagent events.
2. **app-server** can enhance token, context, plan, and thread state when the
   current Codex version supports it.
3. **Local probes** provide cached Git, time, version, and terminal state.

app-server support is optional. Failure falls back to Hooks, then local-only
state, without preventing Codex from starting.

For local development, add and install the bundled event adapter, then review
and trust its commands from `/hooks` in a new Codex session:

```text
codex plugin marketplace add /absolute/path/to/codex-cli-statusline/integrations
codex plugin add codexline-events@codexline-local
```

The adapter is inert unless Codex was launched by Codexline; event datagrams are
loopback-only, session-token authenticated, bounded, and never contain transcript
contents in Codexline state.

## Compatibility target

| Environment | Display backend | Fallback |
| --- | --- | --- |
| macOS | POSIX PTY + ANSI | Direct Codex execution |
| Linux | POSIX PTY + ANSI | Direct Codex execution |
| Windows 10+ | ConPTY + Virtual Terminal | Direct Codex execution |
| WSL | Linux PTY backend | Direct Codex execution |
| tmux / Zellij | Same PTY renderer | ASCII or no overlay |
| Non-TTY, CI, pipes | Overlay disabled | Original Codex behavior |

Target terminals include Terminal.app, iTerm2, Windows Terminal, Kitty,
WezTerm, Alacritty, VS Code, JetBrains, Warp, tmux, and Zellij. Compatibility
claims will be promoted from *target* to *verified* only after automated and
manual testing.

## Performance budget

| Path | Target |
| --- | ---: |
| Wrapper work before starting Codex | `< 20 ms` typical |
| Added terminal relay latency | `< 1 ms` |
| Status render | `< 0.5 ms` typical |
| Idle CPU without animation | Near zero |
| Default / maximum redraw rate | `8 / 20 FPS` |
| Hook execution | `< 5 ms` typical |
| Per-session memory | `< 20 MiB` target |

Every queue, protocol frame, subprocess, external module, and log buffer will
be bounded. Performance changes require before/after measurements.

## Safety and privacy

- No prompt, response, command-output, or transcript collection.
- No parsing of private Codex SQLite tables or rollout formats.
- No modification of the official Codex installation.
- Dynamic terminal text is sanitized against control-sequence injection.
- Local sockets and named pipes are restricted to the current user/session.
- External modules use argv arrays, timeouts, output caps, and concurrency
  limits; shell execution is opt-in.
- Installation and removal are reversible.

The invariant is simple: **Codex must remain usable when Codexline fails.**

## Project status

- [x] Architecture and compatibility design
- [x] Guided and advanced configuration UX
- [x] GitHub README visual direction
- [x] M1 prototype — PTY/ConPTY launch, resize, status row, direct fallback
- [ ] M1 hardening — signals, terminal-mode fixtures, Windows verification
- [x] M2 — Codex Hooks plugin and state engine
- [ ] M3 — app-server enhancement, modules, themes, and live configuration
- [ ] M4 — signed cross-platform installers, compatibility matrix, and `1.0`

See [DESIGN.md](DESIGN.md) for the implementation specification.

## Contributing

The repository is preparing for public development. Before changing
architecture or implementation, read:

- [AGENTS.md](AGENTS.md) — engineering, safety, testing, and GitHub workflow;
- [DESIGN.md](DESIGN.md) — product architecture and performance budgets.

Please open an issue or design note before changing PTY ownership, public state
schemas, plugin protocols, updater trust, or public configuration formats.

## Inspiration

The guided configuration and live-preview philosophy is inspired by
[Claude HUD](https://github.com/jarrodwatts/claude-hud). Codexline uses a
different process architecture because Codex does not currently expose a
command-backed custom status-line provider.

## License

Codexline is available under the [MIT License](LICENSE).
