# AGENTS.md

This file is provided in English, Simplified Chinese, and Japanese. All three
versions are intended to have the same meaning. If wording differs, follow the
stricter safety, compatibility, testing, or review requirement.

本文件提供英文、简体中文和日文版本，三种版本具有同等含义。如表述存在差异，
应采用对安全性、兼容性、测试或审查要求更严格的版本。

このファイルは英語、簡体字中国語、日本語で提供され、3つの版は同じ意味を
持ちます。表現に差異がある場合は、安全性、互換性、テスト、レビューについて
より厳格な要件に従ってください。

---

# English

This section defines the default working agreement for contributors and coding
agents. It applies to the entire repository unless a more specific `AGENTS.md`
exists in a subdirectory.

## Project mission

Build **Codexline**, a fast, attractive, configurable, cross-platform companion
status line for the official Codex CLI.

The repository and package name is `codex-cli-codexline`; the product and
installed command remain `Codexline` and `codexline`.

Codexline wraps the official Codex process with a PTY on Unix-like systems and
ConPTY on Windows, reserves terminal space for its renderer, and gathers state
through supported Codex extension surfaces. It must not become a fork or
partial reimplementation of Codex.

Read `DESIGN.md` before architectural work. If implementation and design
diverge, update the design in the same change or document why the divergence is
temporary.

## Non-negotiable invariants

1. **Codex availability comes first.** Failures must degrade to passthrough or
   direct execution whenever technically safe.
2. Never patch an installed Codex binary or files managed by Homebrew, npm,
   winget, Cargo, or another installer.
3. Never parse private Codex SQLite tables or treat transcript/rollout JSONL as
   a stable API.
4. Protocol integrations must ignore unknown methods and fields. Missing data
   hides a segment rather than crashing or showing a misleading zero.
5. Do not infer Codex state by scraping terminal text. Limited ECMA-48 mode
   observation for rendering correctness is allowed.
6. Preserve arguments, relevant environment, input, resize, signals, and the
   official Codex exit code.
7. Restore terminal modes, cursor, styles, scrolling region, and reserved lines
   after normal exits, errors, panics, interrupts, suspends, and child failure.
8. Put OS-specific code behind explicit Unix and Windows interfaces with shared
   behavioral tests.
9. No telemetry by default. Never collect prompts, responses, commands, file
   contents, credentials, or transcripts.

## Architecture boundaries

Keep these responsibilities separate:

- `process`: Codex discovery, PTY/ConPTY, lifecycle, signals, resize, exit code.
- `relay`: low-latency terminal byte transport; it must not wait on rendering,
  Git, configuration, or protocol parsing.
- `sources`: Hooks, optional app-server events, and cached local probes.
- `state`: versioned events and immutable display-independent snapshots.
- `render`: responsive layout, Unicode width, themes, sanitization, drawing.
- `config`: schema, validation, migration, hot reload, last-known-good fallback.

The relay must remain usable when all sources and rendering are disabled.

## Compatibility

- Primary targets: macOS, Linux, Windows 10+, and WSL.
- Treat WSL as Linux; native Windows uses ConPTY.
- Non-TTY streams, `TERM=dumb`, CI, redirection, unsupported terminals, and
  explicit bypass requests must run without an overlay.
- Hooks are the stable Codex integration. app-server is optional until its
  lifecycle is documented as stable and must have a fast fallback.
- Default themes may use common Unicode but must not require a Nerd Font.
  ASCII and monochrome are first-class modes.

For terminal or platform workarounds, document the capability or bug, detection
method, fallback, and regression test.

## Performance

Treat `DESIGN.md` budgets as acceptance criteria.

- Buffer PTY traffic; do not process it byte by byte without necessity.
- Never run Git, network requests, external modules, config parsing, or
  rendering on the relay path.
- Prefer event-driven updates; stop unnecessary timers.
- Coalesce redraws and enforce a hard frame-rate limit.
- Cache probes, merge duplicate work, and enforce timeouts.
- Bound queues, frames, subprocess output, and log buffers.
- Update benchmarks for startup, relay, redraw, idle CPU, or memory changes.

Record measurement methods for performance claims. Do not sacrifice terminal
recovery or correctness for a microbenchmark.

## Security and privacy

- Treat events, Git metadata, paths, config, module output, and terminal data as
  untrusted.
- Remove unauthorized OSC, DCS, APC, CSI, and control characters from dynamic
  rendered text.
- Scope sockets and named pipes to the current user and session.
- Prefer argv arrays. Shell execution requires explicit opt-in and warning.
- Bound external commands by time, output size, and concurrency.
- Do not expose environment variables, tokens, prompts, responses, or command
  output in diagnostics.
- Installation and removal must be reversible. Never overwrite user files
  without explicit consent and a recoverable backup.

Flag terminal escape handling, process execution, update signatures, IPC, and
shell integration as security-sensitive in review.

## Configuration and state

- Explicitly version public configuration and serialized state.
- Preserve unknown keys during migration where practical.
- Fully validate a candidate before atomically replacing the active snapshot.
- Keep the last-known-good configuration after hot-reload failure.
- Document defaults and handle missing fields safely.
- Breaking changes require migration, release notes, and old/new format tests.

## Code quality

Rust is the expected implementation language unless `DESIGN.md` is amended.
Once a Cargo workspace exists, run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Until commands exist, do not document placeholders as working commands.

- Prefer explicit lifecycle types over loosely related booleans.
- Isolate and document unsafe code with focused tests.
- Do not panic on runtime input or recoverable platform failures.
- Add context to internal errors and keep user messages concise.
- Comments explain invariants and platform constraints, not obvious syntax.
- Do not add a full terminal emulator, async runtime, shell framework, or web UI
  without measured evidence that the current architecture is insufficient.

## Testing

- Unit-test reducers, layout, config, sanitization, and Unicode width.
- Use golden tests with explicit widths and color capabilities for rendering.
- Test PTY/ConPTY input, bulk output, resize, signals, child failure, and exit
  codes with fixtures.
- Test alternate screen, clear/reset, cursor visibility, and abnormal recovery.
- Fuzz protocol parsers; unknown or partial messages must not panic.
- Platform fixes need regression coverage or a written reason why it is
  infeasible.
- Tests must not require a real account, paid calls, or user Codex history.

Use fake Codex fixtures and capability detection in CI. Keep live tests opt-in.

## Documentation and GitHub workflow

Update docs with changes to installation, shim behavior, supported platforms,
configuration, themes, integrations, security, privacy, or fallback behavior.
Examples must distinguish implemented behavior from proposals.

- Keep commits focused with imperative, descriptive subjects.
- Do not mix generated artifacts or unrelated formatting with functional work.
- Preserve unrelated contributor changes.
- Never commit secrets, transcripts, local diagnostics, machine identifiers, or
  editor state.
- PRs state the outcome, platforms tested, fallback, security impact, and
  verification. Performance PRs include before/after measurements.
- Do not push, publish, release, or open PRs without explicit user or maintainer
  authorization.

Open an issue or design note before changing PTY ownership, public state schema,
plugin protocol, updater trust, or public configuration formats.

## Definition of done

A change is complete when the behavior works, Codex remains usable on failure,
relevant platform and terminal cases are handled, tests and docs are updated,
verification passes, and the handoff identifies tested platforms and remaining
uncertainty.

---

# 简体中文

本节定义贡献者和编码代理的默认协作规则。除非子目录中存在更具体的
`AGENTS.md`，否则本节适用于整个仓库。

## 项目使命

构建 **Codexline**：一个快速、美观、易配置、跨平台的官方 Codex CLI 伴生状态栏。

仓库名和软件包名为 `codex-cli-codexline`；产品名与安装后的命令名保持为
`Codexline` 和 `codexline`。

Codexline 在类 Unix 系统上通过 PTY、在 Windows 上通过 ConPTY 包装官方 Codex
进程，为渲染器保留终端空间，并通过 Codex 支持的扩展接口获取状态。它不得演变成
Codex 的 fork 或局部重实现。

进行架构修改前必须阅读 `DESIGN.md`。实现与设计发生偏离时，应在同一变更中更新
设计文档，或说明该偏离为何只是暂时的。

## 不可破坏的约束

1. **Codex 可用性优先。** 发生故障时，只要技术上安全，就必须降级为透明转发或
   直接执行官方 Codex。
2. 禁止 patch 已安装的 Codex 二进制，禁止修改 Homebrew、npm、winget、Cargo 或
   其他安装器管理的文件。
3. 禁止解析 Codex 私有 SQLite 表，禁止把 transcript/rollout JSONL 当作稳定 API。
4. 协议集成必须忽略未知 method 和字段。数据缺失时隐藏 segment，不得崩溃或显示
   具有误导性的零值。
5. 禁止通过抓取终端文本推断 Codex 状态。为了正确渲染，可以有限观察 ECMA-48
   终端模式。
6. 必须保留参数、相关环境、输入、resize、信号以及官方 Codex 的退出码。
7. 正常退出、错误、panic、中断、挂起和子进程失败后，都必须恢复终端模式、光标、
   样式、滚动区域和保留行。
8. 操作系统特定代码必须位于清晰的 Unix/Windows 接口之后，并具有共享行为测试。
9. 默认不启用遥测。不得收集 prompt、response、命令、文件内容、凭据或 transcript。

## 架构边界

保持以下职责分离：

- `process`：Codex 发现、PTY/ConPTY、生命周期、信号、resize、退出码。
- `relay`：低延迟终端字节转发；不得等待渲染、Git、配置或协议解析。
- `sources`：Hooks、可选 app-server 事件和带缓存的本地探针。
- `state`：版本化事件和与显示无关的不可变快照。
- `render`：响应式布局、Unicode 宽度、主题、净化和终端绘制。
- `config`：schema、验证、迁移、热更新和最后有效配置回退。

关闭所有状态源和渲染功能后，relay 仍必须可用。

## 兼容性

- 主要目标：macOS、Linux、Windows 10+ 和 WSL。
- WSL 按 Linux 处理；原生 Windows 使用 ConPTY。
- 非 TTY、`TERM=dumb`、CI、重定向、未知终端和显式旁路请求必须无覆盖层运行。
- Hooks 是稳定 Codex 集成层。app-server 在生命周期正式稳定前只是可选增强，并
  必须具备快速回退。
- 默认主题可使用常见 Unicode，但不能要求 Nerd Font；ASCII 和单色模式是一等功能。

增加终端或平台 workaround 时，必须记录目标能力/缺陷、检测方法、回退行为和
回归测试。

## 性能

把 `DESIGN.md` 中的性能预算视为验收标准。

- 对 PTY 流量使用缓冲，无必要时不得逐字节处理。
- relay 路径不得运行 Git、网络请求、外部模块、配置解析或渲染。
- 优先使用事件驱动更新，停止无必要的定时器。
- 合并重复重绘并设置严格帧率上限。
- 缓存探针、合并重复任务并执行超时。
- 限制队列、协议帧、子进程输出和日志缓冲区大小。
- 影响启动、转发、重绘、空闲 CPU 或内存的修改必须更新 benchmark。

声称性能提升时必须记录测量方法。不得为了微基准牺牲终端恢复和正确性。

## 安全与隐私

- 将事件、Git 元数据、路径、配置、模块输出和终端数据视为不可信输入。
- 从动态渲染文本中移除未授权的 OSC、DCS、APC、CSI 和控制字符。
- socket 和 Named Pipe 必须限制为当前用户和当前会话。
- 优先使用 argv 数组；shell 执行必须显式启用并显示警告。
- 对外部命令限制时间、输出大小和并发数。
- 诊断信息不得暴露环境变量、token、prompt、response 或命令输出。
- 安装和卸载必须可逆；未经明确同意和可恢复备份，不得覆盖用户文件。

涉及终端转义、进程执行、更新签名、IPC 或 shell 集成的修改，在审查中必须标记为
安全敏感。

## 配置和状态

- 对公开配置和序列化状态进行显式版本化。
- 可行时在迁移过程中保留未知配置项。
- 候选配置必须完整验证后再原子替换活动快照。
- 热更新失败时保留最后一份有效配置。
- 记录默认值，并安全处理字段缺失。
- 破坏性修改必须包含迁移、发布说明和新旧格式测试。

## 代码质量

除非修改 `DESIGN.md`，预期实现语言为 Rust。Cargo workspace 建立后运行：

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

命令实际存在前，不得把占位命令写成可运行命令。

- 使用明确的生命周期类型，避免松散关联的布尔值。
- 隔离并记录 unsafe 代码，添加针对性测试。
- 面对运行时输入和可恢复平台错误时不得 panic。
- 内部错误应包含上下文，用户消息应保持简洁。
- 注释解释约束和平台原因，不复述明显语法。
- 没有测量证据证明现有架构不足时，不得引入完整终端模拟器、异步 runtime、
  shell framework 或 Web UI。

## 测试

- 对 reducer、布局、配置、净化和 Unicode 宽度编写单元测试。
- 渲染使用明确宽度和颜色能力的 golden tests。
- 使用 fixture 测试 PTY/ConPTY 输入、大量输出、resize、信号、子进程失败和退出码。
- 测试 alternate screen、clear/reset、光标可见性和异常恢复。
- 对协议解析器进行 fuzz；未知或不完整消息不得造成 panic。
- 平台修复必须有回归测试，无法自动化时需书面说明。
- 测试不得要求真实账号、付费调用或用户 Codex 历史。

CI 使用假的 Codex fixture 和能力检测；真实环境 smoke test 必须显式启用。

## 文档与 GitHub 工作流

修改安装、shim、支持平台、配置、主题、集成、安全、隐私或回退行为时，同步更新
文档。示例必须区分已实现功能和提案。

- commit 应聚焦，标题使用祈使语气并清楚描述内容。
- 功能修改不得混入生成产物或无关格式化。
- 保留其他贡献者的无关修改。
- 禁止提交 secret、transcript、本地诊断、机器标识和编辑器状态。
- PR 应说明结果、测试平台、回退行为、安全影响和验证；性能 PR 应提供前后数据。
- 未经用户或维护者明确授权，不得 push、发布、发版或创建 PR。

修改 PTY 所有权、公开状态 schema、插件协议、更新信任模型或公开配置格式前，优先
创建 issue 或设计说明。

## 完成定义

只有当功能正常、故障时 Codex 仍可用、相关平台和终端边界得到处理、测试和文档已
更新、验证通过，并且交付说明列出已测平台与剩余不确定性时，修改才算完成。

---

# 日本語

この節は、コントリビューターおよびコーディングエージェントの標準作業規約を
定義します。サブディレクトリにより具体的な `AGENTS.md` がない限り、リポジトリ
全体に適用されます。

## プロジェクトの目的

公式 Codex CLI 向けの高速で美しく、設定しやすいクロスプラットフォーム対応の
コンパニオンステータスライン **Codexline** を構築します。

リポジトリ名とパッケージ名は `codex-cli-codexline`、製品名とインストール後の
コマンド名は `Codexline` と `codexline` のままです。

Codexline は Unix 系で PTY、Windows で ConPTY を使って公式 Codex プロセスを
ラップし、描画用の端末領域を予約し、Codex がサポートする拡張インターフェース
から状態を取得します。Codex の fork や部分的な再実装にしてはいけません。

アーキテクチャを変更する前に `DESIGN.md` を読んでください。実装と設計が異なる
場合は、同じ変更で設計文書を更新するか、その差異が一時的である理由を記録します。

## 変更してはならない原則

1. **Codex の可用性を最優先します。** 技術的に安全であれば、障害時には
   パススルーまたは公式 Codex の直接実行へフォールバックします。
2. インストール済み Codex や Homebrew、npm、winget、Cargo などが管理する
   ファイルを patch してはいけません。
3. Codex の非公開 SQLite テーブルを解析せず、transcript/rollout JSONL を安定
   API として扱いません。
4. プロトコル連携は未知の method と field を無視します。データ欠損時は segment
   を隠し、クラッシュや誤解を招くゼロ表示を避けます。
5. 端末テキストのスクレイピングで Codex の状態を推測してはいけません。描画を
   正しく行うための限定的な ECMA-48 モード監視は許可されます。
6. 引数、必要な環境、入力、resize、signal、公式 Codex の終了コードを保持します。
7. 正常終了、エラー、panic、割り込み、suspend、子プロセス障害の後に、端末モード、
   カーソル、スタイル、スクロール領域、予約行を復元します。
8. OS 固有コードは明確な Unix/Windows インターフェースの背後に置き、共通の
   振る舞いテストを用意します。
9. telemetry はデフォルトで無効です。prompt、response、command、ファイル内容、
   認証情報、transcript を収集しません。

## アーキテクチャ境界

次の責務を分離します。

- `process`: Codex 検出、PTY/ConPTY、ライフサイクル、signal、resize、終了コード。
- `relay`: 低遅延の端末バイト転送。描画、Git、設定、プロトコル解析を待ちません。
- `sources`: Hooks、任意の app-server event、キャッシュ付きローカル probe。
- `state`: versioned event と表示非依存の immutable snapshot。
- `render`: responsive layout、Unicode 幅、theme、sanitization、端末描画。
- `config`: schema、validation、migration、hot reload、last-known-good fallback。

すべての source と描画を無効にしても relay は動作しなければなりません。

## 互換性

- 主対象は macOS、Linux、Windows 10+、WSL です。
- WSL は Linux として扱い、ネイティブ Windows では ConPTY を使います。
- 非 TTY、`TERM=dumb`、CI、redirect、未対応端末、明示的 bypass は overlay なしで
  実行します。
- Hooks を安定した Codex 連携層とします。app-server は lifecycle が安定と明記
  されるまで任意機能とし、高速な fallback を必須とします。
- デフォルト theme は一般的な Unicode を利用できますが Nerd Font を必須にせず、
  ASCII と monochrome を第一級モードとして扱います。

端末やプラットフォーム固有 workaround には、対象 capability/bug、検出方法、
fallback、regression test を記録します。

## パフォーマンス

`DESIGN.md` の予算を受入基準として扱います。

- PTY traffic を buffer し、必要なく byte 単位で処理しません。
- relay path で Git、network、外部 module、設定解析、描画を実行しません。
- event-driven update を優先し、不要な timer を停止します。
- redraw をまとめ、厳格な frame-rate 上限を設定します。
- probe を cache し、重複作業を統合し、timeout を設定します。
- queue、frame、subprocess output、log buffer を制限します。
- 起動、relay、redraw、idle CPU、memory に影響する変更は benchmark を更新します。

性能向上を主張する場合は測定方法を記録します。microbenchmark のために端末復元や
正確性を犠牲にしてはいけません。

## セキュリティとプライバシー

- event、Git metadata、path、設定、module output、端末 data を信頼しません。
- 動的描画テキストから許可されていない OSC、DCS、APC、CSI、制御文字を除去します。
- socket と Named Pipe を現在の user と session に限定します。
- argv 配列を優先し、shell 実行には明示的 opt-in と警告を必須とします。
- 外部 command に時間、出力量、並行数の上限を設けます。
- 診断に環境変数、token、prompt、response、command output を出しません。
- install/uninstall は可逆にし、明示的同意と復元可能な backup なしで user file を
  上書きしません。

端末 escape、process execution、update signature、IPC、shell integration の変更は
security-sensitive として review します。

## 設定と状態

- 公開設定と serialized state を明示的に versioning します。
- 可能な限り migration 時に未知の key を保持します。
- candidate 全体を検証してから active snapshot を atomic に置き換えます。
- hot reload 失敗時は last-known-good 設定を維持します。
- default を文書化し、欠損 field を安全に処理します。
- breaking change には migration、release note、新旧 format test が必要です。

## コード品質

`DESIGN.md` を変更しない限り、実装言語は Rust を想定します。Cargo workspace の
作成後は次を実行します。

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

command が実在する前に placeholder を動作する command として記載しません。

- 関連の弱い boolean より明確な lifecycle 型を使います。
- unsafe code を分離、文書化し、対象を絞った test を追加します。
- runtime input や回復可能な platform error で panic しません。
- 内部 error に context を付け、user-facing message は簡潔にします。
- comment は自明な構文ではなく invariant や platform constraint を説明します。
- 現行設計が不十分という測定根拠なしに、完全な terminal emulator、async runtime、
  shell framework、Web UI を追加しません。

## テスト

- reducer、layout、config、sanitization、Unicode 幅を unit test します。
- 描画には端末幅と色 capability を明示した golden test を使います。
- fixture で PTY/ConPTY の入力、大量出力、resize、signal、child failure、終了コードを
  test します。
- alternate screen、clear/reset、cursor visibility、異常終了後の復元を test します。
- protocol parser を fuzz し、未知または不完全な message で panic させません。
- platform 修正には regression test、または自動化できない理由の記録が必要です。
- test は実 account、有料 model call、user の Codex history を必要としません。

CI では fake Codex fixture と capability detection を使用し、live test は opt-in に
します。

## ドキュメントと GitHub ワークフロー

install、shim、対応 platform、config、theme、integration、security、privacy、fallback
の変更と同時に文書を更新します。例は実装済み機能と提案を区別します。

- commit を小さく保ち、命令形で内容を明確に表す subject を使います。
- 機能変更に生成物や無関係な formatting を混ぜません。
- 他の contributor による無関係な変更を保持します。
- secret、transcript、local diagnostics、machine identifier、editor state を commit
  しません。
- PR には成果、test platform、fallback、security impact、検証内容を記載し、性能
  PR には変更前後の測定を含めます。
- user または maintainer の明示的許可なしに push、publish、release、PR 作成を
  行いません。

PTY ownership、公開 state schema、plugin protocol、updater trust、公開 config format
を変更する前に issue または design note を作成します。

## 完了条件

要求された動作が機能し、障害時も Codex が利用可能で、関連する platform/terminal
case が処理され、test と文書が更新され、検証が成功し、引き渡し時に test 済み
platform と残る不確実性が記載されて初めて変更は完了です。
