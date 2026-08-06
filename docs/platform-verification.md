# Platform verification

This file records evidence behind the compatibility table. A compile check is
never treated as a runtime verification.

## 2026-08-06

### macOS arm64

- Local interactive development host.
- `cargo fmt --all -- --check` passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passed.
- 53 unit tests, 4 passthrough/shim tests and 1 native PTY smoke test passed.
- Release build and local Cargo installation passed.
- Multiple terminal emulators were exercised manually during development.

### Debian 12 x86_64

- Native Debian 12 host with kernel 6.1 and glibc 2.36.
- Rust 1.85.1, the declared minimum supported Rust version.
- 53 unit tests, 4 passthrough/shim tests and 1 native PTY smoke test passed.
- `cargo build --release --locked` passed.
- `codexline preview --width 100` rendered the full simulated HUD.
- A 100×24 SSH TTY launched a fake Codex child through the POSIX PTY backend.
- The HUD reserved and rendered three rows, refreshed elapsed state, returned
  the child exit code, cleared its rows, restored the scroll region and showed
  the cursor.
- A real Ctrl+C input stopped the long-running fixture and restored the
  terminal immediately.
- The 100×30 configuration TUI entered the alternate screen, rendered its live
  preview, handled Esc without saving and restored the original screen.

This run exposed Rust syntax newer than the declared 1.85 minimum. The let-chain
expressions were rewritten and the complete suite then passed on Rust 1.85.1.
CI now contains a dedicated MSRV job to prevent recurrence.

### Windows 10/11 x64

- GitHub Actions runs the complete workspace on a native `windows-latest` VM.
- The integration suite opens the system `portable-pty` backend, starts a native
  shell through ConPTY, verifies bidirectional input/output and performs bounded
  master/child teardown.
- Unit and passthrough tests also cover configuration, relay ordering, renderer
  diffing, argument forwarding and owned-shim safety.
- A manual Windows Terminal audit of keyboard input, resize, Ctrl+C and terminal
  restoration is still pending and is not claimed as verified.

### WSL

- Not runtime-tested yet.
- Native Linux results cover the shared POSIX code path, but do not cover the
  Windows Terminal/WSL boundary. WSL remains a support target rather than a
  verified platform.
