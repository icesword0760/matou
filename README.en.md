<p align="center">
  <img src="assets/logo.png" width="128" alt="Matou logo">
</p>

<h1 align="center">Matou (码头)</h1>

<p align="center">
  <strong>A multi-agent desktop workbench for Claude Code</strong><br>
  Run many Claude Code agents side by side on macOS —<br>
  session management, natural-language collaboration across sessions, DAG visualization, tiered notifications, an agent HUD, and Git worktrees.
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: GPL-3.0" src="https://img.shields.io/github/license/icesword0760/matou?color=blue"></a>
  <img alt="Platform: macOS" src="https://img.shields.io/badge/platform-macOS-000000?logo=apple&logoColor=white">
  <img alt="Node 22.16 or newer" src="https://img.shields.io/badge/node-%E2%89%A5%2022.16-339933?logo=node.js&logoColor=white">
  <img alt="Electron 43" src="https://img.shields.io/badge/electron-43-47848F?logo=electron&logoColor=white">
  <img alt="Status: early preview" src="https://img.shields.io/badge/status-early%20preview-orange">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#from-a-pile-of-terminals-to-a-manageable-ai-workflow">Core scenarios</a> ·
  <a href="#3-read-the-session-dag-where-did-this-come-from-where-is-it-going">DAG</a> ·
  <a href="#architecture-and-quality">Architecture</a> ·
  <a href="README.md">中文</a>
</p>

<br>

Matou (码头, "the dock") is a desktop workbench for AI-assisted programming. It puts your Claude Code sessions, tasks, branches and context into one recoverable workspace, so you can push several coding agents forward at once and still know what each one is doing, what it needs, and where it branched off from.

> **Status**: early preview, macOS (Apple Silicon) only. Installer on the [Releases](https://github.com/icesword0760/matou/releases/latest) page. The product UI is currently in Chinese.

![Matou workspace with three parallel Claude Code sessions](assets/shots/workspace-demo.png)

> Above: three Claude Code sessions under one task — the left card is implementing a Redis idempotency key (running), the middle one is a regression run waiting for you to confirm an assertion change, and the right one is a review session that read the regression result with `mt read left` before drawing its conclusion. All screenshots come from an isolated demo environment; the project, terminal output and notifications were constructed for the demo.

---

## What you manage is not terminals, it is work in progress

Two AI coding sessions are easy. Once you have ten, what burns attention is rarely the code:

- Which project, which ticket, which branch does this window belong to?
- Which Claude Code has finished, which one is waiting for confirmation, which one failed?
- Do I really have to copy-paste the conclusion from that other session again?
- How do I try a second approach without losing the original context or polluting my working directory?
- After a restart, do tabs, splits, directories, output and agent identity come back where they were?

Matou folds these into one model: **Workspace → Task → Canvas → Session card**. You see goals, relationships, status and next steps instead of a row of indistinguishable terminal windows.

## From a pile of terminals to a manageable AI workflow

### 1. A four-level structure for projects, tasks and agents

| Level | What goes here | When you use it |
|---|---|---|
| **Workspace** | A repository, product or client environment | Keep directories, tasks and notifications isolated across projects |
| **Task** | One deliverable piece of work, e.g. "ship 0.1" | Move work through ready / active / blocked / done instead of hunting for windows |
| **Canvas** | A phase or scene inside a task | Put exploration, implementation and regression on separate tabs to cut visual noise |
| **Session card** | One independent Claude Code agent | Code, review, test or research in parallel; every card keeps its own input, output and status |

Tasks can be created, renamed, reordered and moved on a board; canvases support tabs plus horizontal and vertical splits; session cards can run independently, take focus, pop out into their own window and return to the canvas.

### 2. Ask other cards for information in plain language

When the result lives in another card you do not have to switch, scroll and copy. Tell the current Claude Code:

> "Check how far the tests in the card on the right have got and give me the conclusion."

> "Read the parent session's latest output and compare the risks of plan A and plan B."

> "Let the session on the left keep running the regression and send the result back when it is done."

Matou gives every agent it hosts the ability to identify itself, list related cards, read live screens or history, inspect available commands, and send input to parent, child, left/right neighbours or any named session. Cross-session collaboration stays inside your task structure.

<details>
<summary>Control commands in the current version</summary>

Inside a hosted session: `mt identify`, `mt list`, `mt read`, `mt history`, `mt commands`, `mt send` and `mt key`. Targets can be `self`, `left`, `right`, `parent`, `child:N`, `sibling:N` or a session reference.

</details>

### 3. Read the session DAG: where did this come from, where is it going

Tabs only tell you *which* sessions exist. The session DAG tells you how they relate. When a problem splits into several validation paths, press `Option + Tab` to open the standalone DAG:

1. **Look up and down from the current node**: find the parent, the current session and its children at a glance.
2. **Two kinds of edges**: solid *Fork* edges inherit the conversation; dashed *association* edges create a relationship without inheriting it.
3. **Judge progress without opening a terminal**: nodes show agent type, work status, directory, branch, latest output and child count.
4. **Search and navigate large graphs**: search by name, path, branch or output; zoom, pan and automatic aggregation keep big graphs readable.
5. **Jump back**: clicking a node closes the DAG and focuses that session; stopped nodes stay in the graph so the decision trail survives.

![Session fork DAG](assets/shots/session-dag-demo.png)

> A baseline session forked into plan A and plan B (solid edges, conversation inherited); a shell running the regression hangs off the same parent through an association (dashed). Without opening anything you can see that A is still running, B is waiting for a decision, and the regression exited with code 1.

### 4. Notifications: interrupt you only when it matters

When a background agent finishes, waits for input, asks for help or hits an error, Matou routes the signal to its card and bubbles it up through canvas, task and workspace. The card you are looking at stays quiet; clicking a notification takes you straight to where it happened.

The notification center shows a summary, the source path and a sound toggle — built for running implementation, tests and review sessions at the same time.

### 5. The HUD: stop asking "where are you now?"

The bottom HUD keeps everything that affects your next decision in one line of sight:

- Model, permission mode, context window and usage percentage
- Session duration, usage windows and reset time
- Working directory, Git branch, dirty state and worktree environment
- Running tools, todo progress, regression runs and MCP errors

![Agent HUD and tiered notification center](assets/shots/agent-hud-notifications-demo.png)

> The notification center labels each entry with "workspace / task", grouped into error, waiting-for-input and completed; a completion from another workspace shows up too. The two cards on the right carry a "new notification" badge, and the HUD at the bottom reports the focused session's model, context usage, weekly usage, todo progress and branch state.

### 6. Fork + Git worktree: compare implementations without fear

Fork an existing session to keep the parent and let the child continue with the inherited conversation. When you need code isolation, bind the branch to a Git worktree:

- the parent keeps guarding the stable implementation;
- children validate plan A and plan B separately;
- every path has a clear session relationship, directory and Git state;
- when validation is done you decide what to merge, keep or stop.

Good for architecture decisions, hard bugs, competing UI designs, parallel code review and risky refactors.

### 7. Use the board to decide what is next, not your memory

The workspace board sorts tasks into **ready, active, blocked, done**. Drag a card to update its status; every card shows its session count so you handle what really needs attention first.

![Workspace board](assets/shots/workspace-board-demo.png)

### 8. Restart recovery and multiple windows: the workspace follows the task

Matou persists tasks, tabs, splits, directories, focus, terminal output and hosted-agent identity. After the window is hidden, the app restarts or crashes, each session type is restored appropriately. A session can also pop out into its own window and return later; the main window and detached windows share the same runtime session.

## Roadmap

**Natural-language structure creation (not implemented).** The current version already lets you read and control other cards in plain language; the next step is describing a task breakdown and letting the agent create workspaces, tasks, canvases and session cards instead of clicking through menus:

> "Create three child cards from these three plans and validate performance, compatibility and rollback separately."

Planned scope: child, sibling and batch card creation; focus, switch, remove, and a preview confirmation before closing.

**Code signing and notarization (not done).** The installer is not signed by Apple yet, so the first launch needs a manual override; an Intel build is not provided yet.

## Who it is for

- Independent developers running several Claude Code sessions at once
- Teams that want to turn AI coding agents from "chat windows" into a manageable workflow
- Complex projects that need a DAG to trace plan branches, context origin and decision paths
- Engineers who develop, test, review and fix in parallel with Git worktrees
- Heavy AI-coding users who care about session recovery, notification tiers and context usage

## Install

1. Download the latest `Matou-<version>-mac-arm64.dmg` (Apple Silicon) from [Releases](https://github.com/icesword0760/matou/releases/latest).
2. Open the DMG and drag **码头** into Applications.
3. The installer is not signed or notarized yet, so macOS will say it cannot verify the developer. Either open **System Settings → Privacy & Security** and click **Open Anyway** at the bottom, or run:

```bash
xattr -dr com.apple.quarantine /Applications/码头.app
```

Make sure the Claude Code CLI is installed and logged in on this machine. New versions are announced inside the app.

## Run from source

### Requirements

- macOS (the only supported platform; Linux and Windows are untested)
- Node.js `>=22.16.0`
- pnpm `10.17.1` (enable with `corepack enable`)
- Claude Code CLI installed and logged in

### Start

```bash
git clone https://github.com/icesword0760/matou.git
cd matou
corepack enable
pnpm install
pnpm dev
```

`pnpm install` fixes the executable bit on node-pty's macOS `spawn-helper`; `pnpm dev` builds the packages and the runtime before launching Electron, so the first start takes a few minutes.

## Common commands

```bash
pnpm test               # unit and integration tests
pnpm typecheck          # workspace-wide type check
pnpm build              # production build
pnpm test:e2e           # full Electron → Runtime → PTY → xterm journeys
pnpm check:identifiers  # brand and naming gate
```

## Project layout

```text
apps/
├── desktop/              Electron main, preload, React renderer, xterm
└── runtime/              UtilityProcess, PTY, sessions, journal, SQLite

packages/
├── contracts/            cross-process protocol and runtime validation
├── domain/               domain types and invariants
└── ui/                   shared UI boundary

docs/
├── architecture/         processes, domain, protocol and ADRs
├── prd/                  product requirements and interaction specs
├── acceptance/           acceptance records and run evidence
└── parity/               behaviour parity matrices

tests/e2e/                real Electron user journeys
```

## Architecture and quality

- Electron main creates and supervises an app-scoped runtime UtilityProcess; terminal data reaches the renderer directly over `MessageChannelMain`.
- The runtime manages PTYs with node-pty and throttles output with a credit window and cumulative ACKs.
- The renderer only consumes rebuildable projections; sessions, hierarchy, persistence and process lifecycle live in the runtime.
- SQLite stores structural metadata; a segmented journal stores terminal output and recovery checkpoints.
- The cross-process protocol uses an exact version handshake and Zod schema validation.
- The renderer keeps `nodeIntegration: false`, `contextIsolation: true`, `sandbox: true`.

Further reading (Chinese): [process model](docs/architecture/process-model.md) · [domain model](docs/architecture/domain-model.md) · [event and stream protocol](docs/architecture/event-and-stream-protocol.md) · [ADR-0001](docs/architecture/adr/0001-app-scoped-utility-process.md)

## Feedback

- Open an [issue](https://github.com/icesword0760/matou/issues) for bugs or ideas.
- For recovery, fork, notification or multi-window problems, please include reproduction steps, your macOS version and shareable demo data.
- Chinese-speaking users can also join the QQ feedback group **454249629** (QR code in the [Chinese README](README.md#反馈与交流)).

## License

Released under the [GNU General Public License v3.0](LICENSE). You are free to use, modify and redistribute it; derivative works must be released under the same license.

---

If Matou saves you a little time hunting for terminals and re-checking context, a ⭐ is appreciated.
