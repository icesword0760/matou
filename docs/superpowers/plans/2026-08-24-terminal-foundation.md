# Terminal Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Establish the approved Electron + React + xterm.js architecture and prove the Renderer-to-UtilityProcess terminal data path with flow control.

**Architecture:** The desktop app owns BrowserWindow lifecycle and transfers MessagePorts but never relays terminal output. An app-scoped Electron UtilityProcess owns PTYs and authoritative runtime state. Renderer state is a projection; terminal bytes, control messages, and semantic events use versioned contracts, while terminal history is designed as segmented files indexed by SQLite.

**Tech Stack:** Electron 43, React 19, TypeScript 7, electron-vite 5, xterm.js 6, node-pty 1.1, pnpm workspaces, Vitest, Playwright.

**Spec:** `/Users/icesword/Documents/AIProjects/matou/docs/architecture/process-model.md`, `/Users/icesword/Documents/AIProjects/matou/docs/architecture/domain-model.md`, `/Users/icesword/Documents/AIProjects/matou/docs/architecture/event-and-stream-protocol.md`

## Global Constraints

- The legacy project at `/Users/icesword/Documents/AIProjects/kookey` is read-only reference material for this implementation.
- Electron renderers keep `nodeIntegration: false`, `contextIsolation: true`, and sandboxing enabled.
- Terminal byte traffic travels directly between Renderer and UtilityProcess after Main transfers MessagePorts.
- Main handles privileged desktop control and channel establishment, not terminal-byte forwarding.
- Runtime V1 is app-scoped and is restarted when the exact protocol version does not match.
- Runtime journals PTY output before it becomes eligible for delivery to a Renderer.
- xterm.js objects never become authoritative domain state.
- No terminal-engine abstraction package is introduced in V1.

---

### Task 1: Architecture Contracts

**Files:**
- Create: `docs/architecture/process-model.md`
- Create: `docs/architecture/domain-model.md`
- Create: `docs/architecture/event-and-stream-protocol.md`
- Create: `docs/architecture/adr/0001-app-scoped-utility-process.md`

**Interfaces:**
- Consumes: approved decisions from the product architecture discussion and the PRDs under `智能体终端升级/`.
- Produces: process ownership rules, domain aggregate definitions, and protocol messages used by later tasks.

- [x] **Step 1: Write the process model**

Document Desktop Main, Preload, Renderer, Runtime UtilityProcess, direct MessagePort transfer, crash/reload behavior, security boundaries, and the V1 version restart rule. Include Mermaid sequence and process diagrams.

- [x] **Step 2: Write the domain model**

Define Workspace, Task, Session, Relation, ExecutionContext, Worktree, Annotation, Artifact, ValidationRun, and the three anchor variants. State aggregate ownership and deletion/lifecycle invariants.

- [x] **Step 3: Write the event and stream protocol**

Define the handshake, control plane, terminal data frames, acknowledgements, credits, replay requests, semantic events, ordering, and failure behavior. Separate Journal sequence from xterm buffer coordinates.

- [x] **Step 4: Record the UtilityProcess ADR**

Capture the decision, alternatives, consequences, and the trigger for migrating to a cross-app daemon.

- [x] **Step 5: Verify documentation consistency**

Run:

```bash
rg -n "TBD|TODO|terminal-api|Main.*转发终端|streamOffset.*fingerprint" docs/architecture
```

Expected: no unresolved placeholders, no terminal-api package, and no obsolete universal stream-offset anchor.

---

### Task 2: Greenfield Monorepo Scaffold

**Files:**
- Create: `package.json`
- Create: `pnpm-workspace.yaml`
- Create: `tsconfig.base.json`
- Create: `vitest.workspace.ts`
- Create: `.gitignore`
- Create: `apps/desktop/package.json`
- Create: `apps/desktop/electron.vite.config.ts`
- Create: `apps/desktop/tsconfig.json`
- Create: `apps/runtime/package.json`
- Create: `apps/runtime/tsup.config.ts`
- Create: `apps/runtime/tsconfig.json`
- Create: `packages/contracts/package.json`
- Create: `packages/contracts/tsup.config.ts`
- Create: `packages/domain/package.json`
- Create: `packages/domain/tsup.config.ts`
- Create: `packages/ui/package.json`
- Create: `packages/ui/tsup.config.ts`

**Interfaces:**
- Consumes: package boundaries from Task 1.
- Produces: `pnpm install`, `pnpm build`, and `pnpm test` workspace commands.

- [x] **Step 1: Add workspace configuration**

Set private ESM workspace metadata and scripts for package builds, runtime builds, desktop builds, unit tests, type checks, and E2E tests.

- [x] **Step 2: Add focused package manifests**

Keep only `domain`, `contracts`, and `ui` as shared packages. Keep xterm integration inside the desktop Renderer and node-pty inside Runtime.

- [x] **Step 3: Install locked dependencies**

Run:

```bash
pnpm install
```

Expected: `pnpm-lock.yaml` is created and native `node-pty` installation completes.

- [x] **Step 4: Verify the workspace graph**

Run:

```bash
pnpm -r list --depth -1
```

Expected: two apps and three shared packages are listed.

---

### Task 3: Versioned Contracts and Runtime Flow Control

**Files:**
- Create: `packages/contracts/src/protocol.ts`
- Create: `packages/contracts/src/protocol.test.ts`
- Create: `packages/contracts/src/index.ts`
- Create: `packages/domain/src/model.ts`
- Create: `packages/domain/src/index.ts`
- Create: `apps/runtime/src/flow-control/credit-window.ts`
- Create: `apps/runtime/src/flow-control/credit-window.test.ts`

**Interfaces:**
- Consumes: message definitions from `event-and-stream-protocol.md`.
- Produces: `PROTOCOL_VERSION`, `parseRendererMessage(value)`, `RuntimeMessage`, and `CreditWindow`.

- [x] **Step 1: Write failing protocol tests**

Test exact-version hello messages, spawn validation, ACK validation, and rejection of unknown messages.

- [x] **Step 2: Run protocol tests and observe RED**

Run:

```bash
pnpm --filter @matou/contracts test
```

Expected: failure because `protocol.ts` does not exist.

- [x] **Step 3: Implement the smallest versioned protocol**

Use discriminated Zod schemas. Keep terminal data as `Uint8Array` and keep semantic events out of terminal frames.

- [x] **Step 4: Run protocol tests and observe GREEN**

Run the same command. Expected: all contract tests pass.

- [x] **Step 5: Write failing credit-window tests**

Test pause above the high watermark, resume at or below the low watermark, duplicate ACK tolerance, and independent session accounting.

- [x] **Step 6: Run runtime tests and observe RED**

Run:

```bash
pnpm --filter @matou/runtime test
```

Expected: failure because `CreditWindow` does not exist.

- [x] **Step 7: Implement `CreditWindow`**

Expose `recordSent(sequence, bytes)`, `acknowledge(throughSequence)`, `unackedBytes`, and `isPaused`. Invoke injected pause/resume callbacks only on state transitions.

- [x] **Step 8: Run runtime tests and observe GREEN**

Run the same command. Expected: all flow-control tests pass.

---

### Task 4: Direct Terminal Data Path

**Files:**
- Create: `apps/runtime/src/runtime-server.ts`
- Create: `apps/runtime/src/session/pty-session.ts`
- Create: `apps/runtime/src/journal/segment-journal.ts`
- Create: `apps/runtime/src/index.ts`
- Create: `apps/desktop/src/main/runtime-host.ts`
- Create: `apps/desktop/src/main/index.ts`
- Create: `apps/desktop/src/preload/index.ts`
- Create: `apps/desktop/src/renderer/index.html`
- Create: `apps/desktop/src/renderer/src/main.tsx`
- Create: `apps/desktop/src/renderer/src/App.tsx`
- Create: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Create: `apps/desktop/src/renderer/src/terminal/terminal.css`
- Create: `tests/e2e/terminal-channel.spec.ts`
- Create: `playwright.config.ts`

**Interfaces:**
- Consumes: `parseRendererMessage`, `RuntimeMessage`, `CreditWindow`, and `PROTOCOL_VERSION` from Task 3.
- Produces: a visible xterm.js surface backed by a node-pty running in an Electron UtilityProcess over a transferred MessagePort.

- [x] **Step 1: Write the failing E2E test**

Launch the Electron app, wait for `[data-testid="runtime-status"]` to become `streaming`, and assert that `[data-testid="smoke-marker"]` becomes `__MATOU_CHANNEL_READY__`.

- [x] **Step 2: Build and run E2E to observe RED**

Run:

```bash
pnpm test:e2e
```

Expected: failure because the desktop and runtime entrypoints do not exist.

- [x] **Step 3: Implement Runtime port attachment and PTY session**

Receive the transferred port from `process.parentPort`, require an exact hello handshake, append output frames to the session journal, apply `CreditWindow`, and handle spawn/input/resize/dispose messages.

- [x] **Step 4: Implement Main channel establishment**

Spawn Runtime with `utilityProcess.fork`, transfer one port to Runtime and one to Renderer, and terminate Runtime when the app exits. Main must not subscribe to terminal data messages.

- [x] **Step 5: Implement isolated Preload port handoff**

Receive the Electron port in the isolated preload world and transfer it into the Renderer main world with a narrowly named window message.

- [x] **Step 6: Implement the React xterm surface**

Create xterm.js in a React effect, send exact-version hello/spawn messages, write `Uint8Array` frames, ACK only from the xterm write callback, and dispose both terminal and session on unmount.

- [x] **Step 7: Run unit, type, build, and E2E verification**

Run:

```bash
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
```

Expected: every command exits 0 and E2E observes the PTY marker through the direct MessagePort channel.

---

### Task 5: Final Architecture Audit

**Files:**
- Modify: `docs/architecture/process-model.md`
- Modify: `docs/architecture/event-and-stream-protocol.md`
- Create: `README.md`

**Interfaces:**
- Consumes: verified implementation details from Tasks 2-4.
- Produces: accurate setup instructions and explicit boundaries between implemented V1 foundation and later product phases.

- [x] **Step 1: Reconcile docs with actual paths and commands**

Update diagrams or names if implementation evidence differs from the initial design.

- [x] **Step 2: Document development commands**

Document install, development, build, unit-test, and E2E commands plus the location of architecture ADRs.

- [x] **Step 3: Run a placeholder and boundary scan**

Run:

```bash
rg -n "TBD|TODO|FIXME|terminal-api|ipcMain.*terminal.data" . \
  --glob '!node_modules/**' --glob '!dist/**' --glob '!out/**' --glob '!pnpm-lock.yaml'
```

Expected: no unresolved architecture placeholders and no Main-process terminal-data forwarding handler.

- [x] **Step 4: Run final verification**

Run:

```bash
pnpm test && pnpm typecheck && pnpm build && pnpm test:e2e && git diff --check
```

Expected: zero failures and no whitespace errors.
