# Contributing to Codexline

[English](#english) · [简体中文](#简体中文)

## English

Thank you for contributing. For architecture or platform work, read
[`DESIGN.md`](DESIGN.md), the accepted decisions in [`docs/adr`](docs/adr), and
the evidence policy in [`docs/platform-verification.md`](docs/platform-verification.md).
Coding agents must also follow [`AGENTS.md`](AGENTS.md).

Before submitting a pull request, run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release
```

Open an issue before changing PTY ownership, public configuration formats,
integration protocols, or updater trust. Keep pull requests focused and state
the platforms tested, fallback behavior, and remaining uncertainty.

Report vulnerabilities privately as described in [`SECURITY.md`](SECURITY.md).

## 简体中文

感谢参与贡献。涉及架构或平台的修改，请先阅读 [`DESIGN.md`](DESIGN.md)、
[`docs/adr`](docs/adr) 中已接受的决策，以及
[`docs/platform-verification.md`](docs/platform-verification.md) 中的验证口径。
编码 Agent 还必须遵循 [`AGENTS.md`](AGENTS.md)。

提交 Pull Request 前请运行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release
```

修改 PTY 所有权、公开配置格式、集成协议或更新信任链前，请先建立 Issue。Pull
Request 应保持范围聚焦，并说明已测试平台、降级行为和仍存在的不确定性。

安全漏洞请按照 [`SECURITY.md`](SECURITY.md) 中的方式私密报告。
