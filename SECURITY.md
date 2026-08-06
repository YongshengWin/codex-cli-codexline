# Security policy

## Supported versions

Security fixes are applied to the latest published release and `main`.

## Reporting a vulnerability

Please use GitHub's private **Report a vulnerability** flow for this repository.
Do not include secrets, private transcripts, access tokens or proprietary source
code in a public issue.

Useful reports include the affected Codexline version, operating system,
terminal, reproduction steps and the security boundary that was crossed.

## Trust boundaries

Codexline runs the user's existing official Codex executable. It never patches
that installation. Release installers verify SHA-256 checksums, the optional
`codex` shim is created only in a Codexline-owned user directory, and unrelated
files are refused. The live relay binds to loopback and forwards unknown
app-server messages unchanged.
