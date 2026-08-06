# Codexline 设计规格

Codexline 是一个跨平台的 Codex CLI 伴生程序。它不替换 Codex、不修改 Codex
安装目录，也不依赖 Codex TUI 的文本布局。它通过 PTY/ConPTY 启动官方 Codex，
为子进程保留少一行终端高度，并在真实终端的最后一行绘制状态栏。

本文定义第一版可实现架构。项目正式名称为 `codex-cli-codexline`，面向用户的
产品名与命令名保持为 `Codexline` / `codexline`。

## 1. 产品目标

- Codexline 必须在视觉层级、响应式布局、主题一致性和信息密度上明显优于 Codex
  原生 status line；仅复制原生字段或改变颜色不构成完成。
- 主力 `attached` 模式位于输入框下方并随 composer 移动；`bottom` 模式是稳定的
  跨版本降级，而不是唯一体验。
- 位置与功能解耦：model、turn、tool、context、usage、Git、agents、plan、permissions
  和 integration health 等模块共用同一状态模型与布局系统。
- macOS、Linux、Windows 10+、WSL 上行为一致。
- 在 Terminal.app、iTerm2、Windows Terminal、Kitty、WezTerm、Alacritty、
  VS Code、JetBrains、Warp、tmux 和 Zellij 中工作。
- 用户可继续输入 `codex`，所有参数、环境、信号和退出码保持不变。
- Codex 升级后不需要重新 patch 或重新编译 Codexline。
- 空闲 CPU 接近零；状态刷新不会降低 Codex 流式输出的响应速度。
- 默认主题无需 Nerd Font，同时支持 Unicode、Nerd Font 和纯 ASCII。
- 配置可热更新；错误配置和扩展模块失败不能阻止 Codex 启动。
- 不读取 Codex 私有 SQLite 表，不依赖 transcript/rollout JSONL 格式。

非目标：重新实现 Codex TUI、模型调用、权限系统、会话存储或工具执行。

## 2. 总体架构

```text
                    +-----------------------+
keyboard ---------->|                       |----------> child PTY/ConPTY
                    |      codexline        |               |
terminal <----------|                       |<---------- official codex
                    +----+-------------+----+
                         |             |
                 terminal renderer     state engine
                                       /     |      \
                                  hooks   app-server  local probes
                                           adapter    (git/time/os)
```

Codexline 是官方 Codex 的父进程。假设真实终端为 `120x40`，它给 Codex 创建
`120x39` 的子终端。Codex 只能布局前 39 行，Codexline 使用第 40 行。

程序分成五个边界清晰的 crate/module：

1. `process`：真实 Codex 发现、PTY/ConPTY、信号和退出码。
2. `relay`：无解析、低延迟地双向转发终端字节流。
3. `sources`：Hooks、app-server 和本地探针适配器。
4. `state`：把不同事件归一化为版本化快照。
5. `render`：响应式布局、主题、宽度计算和终端绘制。

## 3. 跨平台进程层

### 3.1 Unix

- macOS/Linux/WSL 使用 POSIX PTY。
- 第一版采用 `portable-pty`，必要时为 macOS 和 Linux 增加小型原生后端。
- 父进程进入 raw mode；子进程拥有独立 session 和 controlling terminal。
- `SIGWINCH` 时读取真实尺寸，将子 PTY 更新为 `rows - reserved_rows`。
- `SIGINT`、`SIGTERM`、`SIGHUP`、`SIGTSTP` 和 `SIGCONT` 按终端程序语义转发。

### 3.2 Windows

- Windows 10 1809+ 使用 ConPTY。
- 启用 Virtual Terminal Processing 后使用与 Unix 相同的 ANSI renderer。
- 使用 Job Object 绑定子进程生命周期，避免启动器异常后遗留 Codex。
- Ctrl+C、窗口 resize 和退出码通过 Windows backend 映射。
- WSL 作为 Linux 环境处理，不走原生 Windows backend。

### 3.3 非交互模式

以下情况完全旁路，直接执行真实 Codex：

- stdin 或 stdout 不是 TTY。
- `TERM=dumb`。
- `codex exec`、管道、CI 或显式 `--no-companion`。
- 终端小于 `40x8`。
- PTY/ConPTY 初始化失败。

旁路是功能，不是错误；必须保留原始退出码。

## 4. Codex 发现与透明启动

安装器提供两种模式：

- `codexline` 命令：最安全，用户显式运行 `codexline run -- ...`。
- `codex` shim：推荐体验，用户仍输入 `codex`。

真实二进制发现顺序：

1. `CODEXLINE_CODEX_BIN` 指定的绝对路径。
2. 安装时保存的、仍然有效的路径。
3. PATH 中排除当前 shim 后的下一个 `codex`。
4. 已知安装器位置仅作为最后的提示，不静默猜测。

发现结果必须满足：不是当前 shim、可执行、`codex --version` 可在短超时内成功。
所有参数和环境原样传递。`codexline doctor` 显示解析过程。

## 5. 状态数据源

### 5.1 稳定层：官方 Hooks

随伴生程序安装的 Codex 插件注册：

- `SessionStart` / `SessionEnd`
- `UserPromptSubmit` / `Stop`
- `PreToolUse` / `PostToolUse`
- `PermissionRequest`
- `PreCompact` / `PostCompact`
- `SubagentStart` / `SubagentStop`

Hook 命令为 `codexline hook`。它从 stdin 读取一次 JSON，通过继承的
`CODEXLINE_EVENT_ENDPOINT` 发送给父进程，然后立即退出。Hook 不产生 stdout，
不等待重试，不在事件路径中运行 Git。

Unix 使用权限为 `0600` 的 Unix datagram socket；Windows 使用当前用户 ACL 的
Named Pipe。消息上限 64 KiB，超限或 socket 不存在时丢弃并返回成功。

Hooks 提供 session、cwd、model、permission mode、turn、tool 和 subagent 状态。

### 5.2 增强层：app-server sidecar 与可选实时中继

M3 同时提供独立、只读的 stdio app-server sidecar，以及显式选择后才启用的
loopback WebSocket proxy。sidecar 是默认路径，只读取 `account/rateLimits/read` 和
滚动更新；它不得成为 Codex TUI 的传输依赖，也不得被当作当前 thread 的
context/token 来源。sidecar 不可用时保留 Hooks/本地探针，不能影响 Codex 会话。

proxy 让 TUI 与 Codexline 观察同一条 app-server thread 协议流，从而采纳
`thread/tokenUsage/updated`，但它处于 Codex 的关键传输路径。建立会话后的连接重置
可能终止官方 TUI，因此保持显式选择；配置 schema v1 自动迁移到 v2 并关闭 proxy。

当当前 Codex 同时支持 `app-server` 和 `--remote`，且用户显式设置
`sources.remote_proxy = true` 时，启用透明代理：

```text
Codex TUI <-> codexline protocol proxy <-> codex app-server
```

代理原样转发协议帧，只复制并容错解析已知通知，例如：

- `thread/status/changed`
- `turn/started` / `turn/completed`
- `item/started` / `item/completed`
- `turn/plan/updated`
- `thread/tokenUsage/updated`

未知 method 和字段必须原样转发并忽略。握手失败、协议不兼容或 app-server
启动超时后，在 Unix 800 ms、Windows 1500 ms 的有界窗口内回退 Hooks/sidecar
模式并正常启动普通 Codex。中继对单方向同时施加 256 帧和 64 MiB 背压上限；
`WouldBlock` 表示帧已进入 tungstenite 内部缓冲区，只有 `WriteBufferFull` 返回的帧
才允许重新排队。Ping/Pong 在各自 hop 本地终止，Close 尽力传播。

连接建立后的中断目前不能无损热切换回原生传输，因此 UI 必须明确显示 `LIVE !`
并清除实时新鲜度；它不能作为启动必需条件。精确 token 只采纳当前连接收到的
`thread/tokenUsage/updated`：进度条使用 `last.inputTokens`，Token 模块使用 `total`
累计计数。

### 5.3 本地探针

本地探针在后台、带缓存执行：

- Git branch、dirty/staged、ahead/behind、worktree。
- Codex 持久化配置中的 model、reasoning、sandbox、approval policy 和 reviewer。
- 当前目录和项目根。
- 会话/当前 turn 耗时。
- Codex 和 Codexline 版本。
- 终端宽度、主机名和时钟（可选）。

Git 与 Codex 配置默认每 3 秒刷新，Git 子命令单次超时 100 ms；相同 repo 的并发请求合并。文件系统
watcher 只用来使缓存失效，不能触发高频完整 `git status`。

每个可能变化的字段必须保留来源/新鲜度语义。独立 sidecar 只能标记为 `ACCOUNT`，
不能标记为当前会话 `LIVE`；启动配置必须显示 `default`，新会话初始 context 必须显示
`start`。当前 turn 开始后若没有 Hooks 或 proxy 提供真值，应隐藏未知数据，不得继续
展示过期值。禁止解析 Codex 屏幕文本来补齐状态。

## 6. 版本化状态模型

内部快照与 renderer 解耦：

```json
{
  "schema_version": 1,
  "session": {
    "id": "thr_xxx",
    "name": null,
    "started_at_ms": 1785940000000
  },
  "codex": {
    "version": "0.146.0",
    "model": "gpt-5.6-sol",
    "reasoning": "high",
    "permission_mode": "default",
    "sandbox": "workspace-write"
  },
  "turn": {
    "id": "turn_xxx",
    "phase": "tool_running",
    "active_tool": "exec_command",
    "started_at_ms": 1785940010000
  },
  "usage": {
    "context_used": 42000,
    "context_size": 200000,
    "context_percent": 21.0,
    "input_tokens": 36000,
    "output_tokens": 6000
  },
  "agents": { "active": 2, "total": 3 },
  "git": {
    "branch": "feat/statusline",
    "dirty": true,
    "staged": 1,
    "modified": 3,
    "ahead": 2,
    "behind": 0
  },
  "capabilities": {
    "hooks": true,
    "app_server": true,
    "token_usage": true
  }
}
```

任何字段都允许未知。Renderer 对缺失字段隐藏 segment，不能显示误导性的零值。

### 6.1 Statusline 能力分层（2026-08 调研）

Codex 原生 footer 已经覆盖 model/reasoning、cwd/project、Git branch、PR、branch
changes、run state、permissions/approval、context used/remaining/window、5h/weekly
limits、token totals、session/thread、Fast/Raw mode、thread title、workspace headline 和
task progress。Codexline 不应简单复制这些字段，而应把多个稳定来源合成一个可响应式
布局的 HUD。

| 数据层 | 可提供的信息 | 稳定性与策略 |
| --- | --- | --- |
| 本地探针 | cwd/project、Git staged/modified/untracked、ahead/behind、stash、linked worktree、其他 worktree、会话耗时、Codexline/Codex 版本 | 默认启用；后台缓存、超时、绝不阻塞 PTY relay |
| Codex Hooks | session、当前 model/cwd、tool start/stop、permission waiting、compact、subagent start/stop、prompt/stop | 默认增强层；事件写入本会话私有 IPC，不解析屏幕 |
| Codex app-server | thread/turn 状态、items、plan、diff、token usage、rate limits、title、background terminals、approval requests | 权威但属于 custom-client 协议；作为高级后端，不能假装可附着到任意已运行 TUI |
| 派生指标 | tool/agent elapsed、token speed、context burn rate、最近错误、变更速率、idle time | 只由权威或稳定输入计算，并标记 estimated/derived |
| 可选集成 | GitHub PR/CI/review、Jujutsu、跨 Codexline 会话/跨 worktree 活动 | 显式启用、独立 TTL；网络与外部 CLI 永不进入渲染热路径 |

推荐模块全集：

- Session：model/provider、reasoning、Fast mode、run state、session/turn elapsed、thread title。
- Capacity：context bar/tokens、5h/weekly limits 与 reset、input/cached/output、compaction count。
- Workspace：project/cwd、Git branch、staged/modified/untracked、ahead/behind、stash、PR/CI、worktree 名称与总数。
- Activity：当前 tool、最近完成 tools、skills/MCP、active agents（type/state/elapsed）、plan/todo、background terminals。
- Safety：sandbox/permission profile、approval mode、等待审批/等待用户输入、降级数据源告警。
- Fleet：本机其他 Codexline 会话、每个 worktree 的 agent/branch/dirty/blocked 状态；这是 Codexline 相比单会话 HUD 的核心差异化能力。

默认不展示系统 RAM/CPU/电量，不从认证文件抓限额，不把估算成本伪装成账单，也不把
Codex TUI 屏幕文本或 session transcript 当作稳定 API。它们可以成为显式 opt-in
诊断模块，但不能是 Full preset 的核心数据源。

## 7. Renderer

Codexline 的 renderer 是独立产品界面，不模仿 Codex 原生 footer。默认设计必须满足：

- 信息具有明确的主次层级；运行状态和风险信息优先于装饰信息。
- 宽终端展示完整上下文，窄终端自动压缩，不依赖手工切换 preset。
- 颜色用于表达状态而非填满界面；默认主题无需 Nerd Font，ASCII 与单色模式仍完整。
- 不连续闪烁、不争抢输入光标、不让动画掩盖真实延迟。
- 缺失数据隐藏模块，降级状态明确标记，不显示误导性的 `0` 或虚构百分比。

首版丰富模块目标：

| 类别 | 模块 |
| --- | --- |
| Session | model、reasoning、session elapsed、Codex version |
| Work | phase、active tool、tool elapsed、permission request |
| Capacity | context used/remaining、tokens、rate/usage warning |
| Project | cwd、project root、Git branch/dirty/ahead/behind |
| Agents | active/total subagents、current agent state |
| Progress | plan step、completed/total、compact state |
| Safety | approval mode、sandbox、integration degradation |

模块丰富度不得进入 relay 热路径；所有数据源必须通过缓存状态快照驱动 renderer。

### 7.0 Placement frontends

```toml
[display]
placement = "auto" # auto | attached | bottom | off
attach_side = "below" # above | below
```

- `attached`：主力体验，在 composer 附近合成 Codexline，并随输入框布局变化。
- `bottom`：占用真实终端最后一行，提供最高兼容性。
- `auto`：优先 attached；定位能力不可信或版本未知时立即回退 bottom。
- `off`：关闭 Codexline 绘制但保留透明进程转发。

原生 Codex status line 与 Codexline 同时显示会产生重复。配置器可以建议禁用
`tui.status_line`，但修改前必须展示 diff、创建可恢复备份并获得明确确认；卸载时只在
用户未自行修改该值的情况下恢复。Codexline 不得静默编辑 Codex 配置。

检测必须遵循 Codex 的公开配置优先级：CLI override、受信任项目配置、profile、用户
配置、系统配置、内置默认。Codexline 不读取私有 trust state；项目层是否生效无法从
公开配置确定时返回 `unknown`。`attached` 不得在检测为 `enabled` 或 `unknown` 时
静默覆盖原生 footer，必须提示、获得确认或回退 `bottom`。

### 7.1 终端所有权

Renderer 不解析 Codex 文本内容，只进行有限的 ECMA-48 模式观察：alternate
screen、cursor visibility、全屏 reset 和 synchronized update。它不根据屏幕文本
推断 Codex 状态。

每次绘制：

1. 合并 16 ms 内的多个状态变化。
2. 生成一行已知显示宽度的 cell 列表。
3. 使用 synchronized output（终端支持时）减少闪烁。
4. 保存光标，移动到真实终端最后一行，清行并输出。
5. 恢复 SGR 和光标状态。

Codex 清屏或切换 alternate screen 后立即补画。若子 TUI 使用 synchronized output，
在其帧结束后用独立的 synchronized transaction 原子恢复全部 HUD 行，不等待常规
刷新周期。常规状态按事件驱动更新；只有 spinner、时钟和耗时启用时才使用定时器。

启动时先预留行但不抢在 Codex 前绘制。renderer 忽略终端能力查询等纯 ANSI 控制流，
在观察到首批可见 Codex 内容并安静 50 ms 后一次性揭示 HUD；仅当子进程完全没有
输出时才在 800 ms 后兜底显示。该门控不得匹配 Codex 文案或依赖界面语言。

### 7.2 响应式布局

每个 segment 声明：

- `priority`：宽度不足时的隐藏顺序。
- `min_width` / `max_width`。
- `truncate = "left|middle|right"`。
- `show_when` 条件。
- compact 和 full 两种表示。

布局先保留高优先级 segment，再按可用宽度展开。宽度使用 Unicode grapheme 和
East Asian Width 计算，不能用字符串字节数。

建议 Full 默认布局分为三条稳定语义泳道：

```text
 ● Codex | gpt-5.6-sol high | exec 8s | 41s
 git:(feat/statusline*) S1 M3 ↑2 | wt:codexline-agent-2 ↗ | codex-cli-codexline
 ctx ███░░ 42% | ↑2/3 agents | 2/4 plan | workspace · ask
```

窄终端自动压缩为：

```text
 ⟳ exec 8s | ctx 42% | main*
```

### 7.3 颜色与字形

颜色能力按 `COLORTERM`、terminfo 和 Windows VT 能力检测：truecolor -> 256 ->
16 -> monochrome。主题可以覆盖，但不能假设用户安装 Nerd Font。

内置字形配置：

- `ascii`：服务器、串口和无 Unicode 环境。
- `unicode`：默认，使用常见符号和 block bar。
- `nerd-font`：显式启用，不自动猜字体。

内置主题至少包括默认的 `inherit`、`ox96f`、`tokyo-night`、`catppuccin-mocha`、
`dracula`、`nord`、`gruvbox`、`rose-pine`、`codex-dark`、`codex-light`、`minimal`、
`mono`。`inherit` 不输出背景色，使用终端调色板保持与 Codex
及用户主题一致；固定背景主题仅在显式选择时启用。默认主题优先可读性，不使用
连续动画和大量 emoji。

### 7.4 Agent Inspector

当官方 app-server 或 Hooks 报告子代理后，`bottom` 前端自动在状态栏下展开最多三条
代理行，显示运行/完成状态、角色、耗时和最新活动。被动展示不接管任何按键；行首
明确显示 `Ctrl+G focus`。用户进入焦点后，`↑/↓` 选择、`Enter` 打开只读详情、
`Esc` 返回或关闭。详情只展示官方协议提供的 prompt、状态和 agent message，不读取
rollout JSONL 或私有 SQLite。

输入路由只识别独立的 `Ctrl+G` 以及面板已聚焦时的导航键。其他时候终端字节必须
原样转发给 Codex。面板行数变化通过 PTY resize 调整保留区；小终端优先保留 Codex
至少四行可用区域并裁剪 Inspector。

## 8. 配置体验

配置路径遵循平台规范：

- Unix：`$XDG_CONFIG_HOME/codexline/config.toml`，默认 `~/.config/codexline/`。
- macOS：同时接受 XDG 路径，避免产生另一套行为。
- Windows：`%APPDATA%\codexline\config.toml`。

最小配置示例：

```toml
version = 2

[launch]
mode = "shim" # shim | explicit
bypass_flag = "--no-companion"

[display]
theme = "inherit"
glyphs = "unicode"
position = "bottom"
refresh_hz = 8

[sources]
app_server = true # isolated read-only capacity sidecar
remote_proxy = false # explicit live-thread relay for exact active-session data

[[layout.left]]
module = "model"
priority = 90

[[layout.left]]
module = "turn"
priority = 100

[[layout.center]]
module = "context"
style = "bar"
priority = 80

[[layout.right]]
module = "agents"
priority = 60

[[layout.right]]
module = "git"
priority = 70
```

提供以下交互命令，用户不需要手写 TOML：

```text
codexline setup          安装插件和可选 shim
codexline config         交互配置器，修改时实时预览
codexline preview        用模拟状态预览主题和宽度
codexline themes         浏览内置主题
codexline doctor         检查 Codex、PTY、Hooks、app-server 和终端能力
codexline uninstall      完整、可逆地移除插件和 shim
```

配置 watcher 使用 debounce 和原子快照。新配置解析失败时保留最后一份有效配置，
并在状态栏显示短暂警告；不能终止 Codex。

### 8.1 双层配置流程

`codexline config` 默认进入固定终端视口的六区键盘编辑器：

```text
Launch <-> Preset <-> Modules <-> Appearance <-> Data <-> Review
```

界面使用 alternate screen，不产生滚屏历史；`←/→` 切区、`↑/↓` 移动、Space/Enter
选择、`S` 保存、Esc 取消。必须能在 `80x24` 终端中完整使用，并始终提供当前配置的
实时预览。配置项使用上方整宽区域，实时预览固定在下方整宽区域；两者之间的分隔栏
显示当前 preset、行数、模块数与数据源，使预览宽度尽量贴近最终 HUD。所有改动先写入
同一内存快照，保存前不修改磁盘。Full、Focus 和 Minimal
是起点而不是锁定模板；用户修改任何单项后，配置状态变为 Custom。窗口小于最低
尺寸时显示 resize 提示；`CODEXLINE_CONFIG_LINEAR=1` 提供 screen reader 和兼容回退。

Modules 不得把全部字段放进单一纵向长列表。它使用 Core、Usage、Workspace、Activity
和 Runtime 五个横向分类；在 Modules 页中 `←/→` 切换分类，`↑/↓` 只遍历当前分类，
Tab/Shift+Tab 返回主步骤导航。分类切换不得改变用户已配置的模块顺序。每个分类必须
同时提供摘要模块与真实字段级模块；字段未知时遵循 omit-on-unknown，不能显示伪造的零。

字段级模块至少覆盖 reasoning、context remaining/used/window、input/cached/output tokens、
5-hour/weekly limits、reset credits、Git dirty/staged/modified/ahead-behind、project root、
agent active/total、thread ID，以及 hooks/app-server 的独立健康状态。

配置器维护 Primary tabs、Modules secondary tabs 和 Options 三层显式焦点。`↑/↓` 在层级
之间进入或返回，并在 Options 内移动；`←/→` 只切换当前获得焦点的 tab 行。Space 只
修改 Options，Enter 必须从任意层级校验并保存。三个层级的焦点样式必须有明显区别。

Launch 步骤必须让用户明确选择启动方式：

1. **Keep `codex` command（推荐）**：安装用户级、可逆的 PATH shim，用户继续输入
   `codex`；不得覆盖官方二进制，必须保留所有参数、相关环境、信号和退出码，并支持
   `codex --no-companion` 绕过 overlay 后直接启动解析到的官方二进制。
2. **Use `codexline` command**：不创建 `codex` shim，用户显式运行 `codexline` 或
   `codexline run -- <codex args>`；shell 中的 `codex` 继续直接指向官方程序。

保存前必须展示 dry-run：计划创建的 shim 路径、解析到的官方 Codex 绝对路径、当前
PATH 优先级、会修改的 shell 配置（如有）以及卸载恢复方式。如果 PATH 顺序使 shim
无法生效，向导应阻止静默成功，并给出精确修复建议。安装清单必须支持无损卸载；
用户已有同名文件时不得覆盖。

用户可以在任意步骤按 `A` 进入高级三栏编辑器：

1. 左侧为 Presets、Layout、Modules、Theme、Compatibility 和 Advanced 分类；
2. 中间编辑模块顺序、优先级、宽/窄可见性、条件、阈值与 compact 表示；
3. 右侧模拟不同宽度、字形能力、context warning、app-server 缺失等场景，并展示
   保存前的 TOML diff。

向导和高级编辑器操作同一个版本化配置快照，切换时不得丢失未保存修改。高级编辑器
返回向导后，应回到 Review 步骤。两种界面都必须完全键盘可操作，并提供 ASCII、
monochrome、reduced-motion 与 screen-reader-friendly 的线性预览。

### 8.2 外部模块

高级用户可以增加命令模块，但默认不经过 shell：

```toml
[[modules.command]]
id = "ticket"
argv = ["my-ticket-status", "--json"]
interval = "5s"
timeout = "100ms"
max_output_bytes = 1024
priority = 20
```

外部模块获得只读状态 JSON，并返回纯文本或受限 JSON。默认移除 ANSI 控制字符；
只有显式 `allow_style = true` 时允许经过白名单解析的 SGR。

## 9. 性能预算

Release 构建目标：

- 在启动 Codex 前增加的 wrapper 工作量：典型小于 20 ms。
- 终端转发：64 KiB 缓冲，避免逐字节处理；额外首字节延迟小于 1 ms。
- 状态渲染：典型小于 0.5 ms，最坏小于 2 ms。
- 空闲 CPU：无时钟/spinner 时接近 0%；有 spinner 时小于 0.2%。
- 单会话 RSS：目标小于 20 MiB，不引入 Web runtime。
- 绘制上限：默认 8 FPS，硬上限 20 FPS。
- Hook：典型小于 5 ms，绝不等待 Git、网络或 app-server。
- 外部模块：独立并发限制、超时、输出上限和熔断器。

实现上不使用完整终端模拟器和 DOM。Relay 路径与 JSON、Git、主题渲染完全隔离；
Codex 大量输出时状态事件不能反向阻塞 PTY reader。

## 10. 安全与隐私

- 本地 socket/pipe 仅当前用户可访问，并包含每会话随机 nonce。
- 动态文本在渲染前移除 OSC、DCS、APC 和未授权 CSI，防止终端注入。
- 不记录 prompt、assistant 内容、命令输出或 transcript。
- debug 日志默认关闭；开启时仍对路径、token 和环境变量做脱敏。
- 外部模块不经过 shell，除非用户显式选择 `shell = true`。
- 所有子命令有时间、输出大小和并发限制。
- app-server 代理只绑定 loopback/本地 socket，不暴露网络监听端口。
- 更新包使用平台签名和发布校验和；自动更新默认只提示，不静默替换。

## 11. 故障处理

优先级始终是“Codex 可用，状态栏其次”：

1. app-server 不可用 -> Hooks。
2. Hooks 不可用 -> 本地 model/version/git/time。
3. 状态采集失败 -> 显示静态 Codexline 标识或隐藏。
4. Renderer 失败 -> 关闭保留行，继续 PTY passthrough。
5. PTY 初始化失败 -> 直接 `exec` 官方 Codex。

使用 RAII terminal guard 和 panic/signal handler 恢复 raw mode、光标、SGR、滚动区和
最后一行。退出码必须是官方 Codex 的退出码。`codexline doctor --report` 生成不含
会话内容的诊断包。

## 12. 安装与发布

首批发布渠道：

- macOS：Homebrew tap + 签名 universal binary。
- Linux：静态或最小动态依赖的 tarball，后续提供 deb/rpm/AUR。
- Windows：winget + 签名 zip/msi。
- cargo-binstall 作为开发者渠道，不要求普通用户安装 Rust。

`setup` 必须展示将要创建的文件，并维护安装清单。shim 默认放在用户级 bin，绝不
覆盖官方 Codex 文件。卸载时只删除清单中由 Codexline 创建且内容未被用户修改的
文件。

## 13. 测试策略

- 状态 reducer、布局、Unicode 宽度和主题使用单元与 golden tests。
- 使用 `vt100` 测试模型验证 resize、clear screen、alternate screen 和光标恢复。
- PTY 集成测试运行假的 Codex fixture，覆盖大量输出、交互输入和异常退出。
- JSON/event parser 做 property tests 和 fuzzing，未知字段永不 panic。
- CI 矩阵：macOS arm64/x64、Linux glibc/musl、Windows x64/arm64。
- 手工兼容矩阵覆盖主要终端、tmux/Zellij、SSH、中文宽字符和 screen reader 模式。
- 至少测试当前 Codex、上一稳定版和最新发布版；协议增强失败必须验证自动降级。

## 14. 里程碑

### M1：可靠终端代理

- POSIX PTY、ConPTY、resize、signal、恢复和透明参数传递。
- 静态/本地状态栏、响应式布局、四个基础主题。
- `doctor` 和旁路模式。

### M2：Codex 插件集成

- Hooks 插件、事件 socket、状态 reducer。
- model、turn、tool、permission、subagent、compact 状态。
- Git 探针与热配置。

### M3：丰富数据

- app-server 透明代理和 capability detection。
- token/context/plan 等增强字段。
- command modules、主题导入和交互配置器。

### M4：发行质量

- 全平台安装器、签名、升级、uninstall。
- 完整终端矩阵、fuzzing、基准和故障注入。
- 发布 `1.0`，稳定配置与状态 schema。

## 15. 将来的原生迁移

如果 Codex 后续支持 command-backed status line provider，Codexline 保留全部状态、
布局、主题和模块能力，只新增一个 stdin/stdout provider frontend。PTY frontend 继续
作为旧版兼容层。用户无需迁移配置。

这使上游采纳成为体验增强，而不是项目成立的前提。
