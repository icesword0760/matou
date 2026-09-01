# Matou Cloud App Updates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Matou 增加不打断终端工作的 macOS 整包云端更新能力，并实现已确认 Mockup 的全部可见状态。

**Architecture:** 主进程中的 `AppUpdateManager` 封装 `electron-updater` 并发布不可变状态，类型化 IPC 将状态桥接到 Renderer。`AppUpdateControl` 根据投影计算的活动会话数决定立即安装或等待空闲，发布配置生成通用 HTTPS 更新源所需的 macOS 产物。

**Tech Stack:** Electron 43、electron-builder 26、electron-updater、React 19、TypeScript、Vitest、Testing Library

**Spec:** `docs/superpowers/specs/2026-09-01-cloud-app-updates-design.md`

## Global Constraints

- macOS arm64/x64 为第一阶段平台。
- 更新检查由主进程执行，Renderer 仅通过类型化 IPC 操作。
- `autoDownload=false`，下载必须由用户触发。
- `autoInstallOnAppQuit=true`，退出时安装已下载版本。
- 活动会话是未归档且状态为 `starting`、`running` 或 `needs-input` 的唯一会话。
- 真实更新仅在打包应用启用；单元测试使用注入适配器。
- 更新源由 `MATOU_UPDATE_BASE_URL` 配置，默认通道为 `stable`。
- 所有安装路径先完成 Runtime 优雅关闭。

---

### Task 1: App update state machine

**Files:**
- Create: `apps/desktop/src/main/app-update-manager.ts`
- Test: `apps/desktop/src/main/app-update-manager.test.ts`

**Interfaces:**
- Produces: `AppUpdateManager`, `AppUpdaterAdapter`, `AppUpdateManagerOptions`
- Consumes: shared `AppUpdateState` and `AppUpdateProgress`

- [ ] Write failing tests for initial state, check events, progress normalization, downloaded state, error state and single install invocation.
- [ ] Run `pnpm --filter @matou/desktop test -- app-update-manager.test.ts` and confirm failures are caused by the missing manager.
- [ ] Implement event binding, immutable state publication, manual download and install preparation.
- [ ] Re-run the focused test and commit the green state.

### Task 2: Typed desktop bridge

**Files:**
- Modify: `apps/desktop/src/shared/desktop-api.ts`
- Modify: `apps/desktop/src/preload/index.ts`
- Modify: `apps/desktop/src/main/index.ts`
- Test: `apps/desktop/src/main/app-update-manager.test.ts`

**Interfaces:**
- Produces: `getAppUpdateState`, `checkForAppUpdates`, `downloadAppUpdate`, `installAppUpdate`, `onAppUpdateState`
- Consumes: singleton `AppUpdateManager`

- [ ] Add shared state and progress types plus exact IPC channel names.
- [ ] Expose typed invoke/subscription methods through the preload bridge.
- [ ] Instantiate the manager after Runtime startup, broadcast state to live windows, schedule the initial and four-hour checks, and route commands.
- [ ] Route installation through a shared `shutdownRuntime()` promise before `quitAndInstall()`.
- [ ] Run desktop typecheck and focused tests; commit.

### Task 3: Update control interaction

**Files:**
- Create: `apps/desktop/src/renderer/src/updates/AppUpdateControl.tsx`
- Create: `apps/desktop/src/renderer/src/updates/AppUpdateControl.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/SceneTabBar.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy.css`

**Interfaces:**
- Produces: `<AppUpdateControl activeSessionCount={number} />`
- Consumes: `window.matouDesktop` update bridge

- [ ] Write failing interaction tests for available, downloading, downloaded with active sessions, idle scheduling, outside click, Escape and errors.
- [ ] Verify focused test failures.
- [ ] Implement the accessible icon, glass popover, progress, active-session warning, install queue and one-time success toast.
- [ ] Count unique active sessions from all session graphs and pass the control into both normal and settings tab bars.
- [ ] Match Mockup spacing, visual hierarchy and reduced-motion behavior.
- [ ] Run component tests and commit.

### Task 4: Release configuration

**Files:**
- Modify: `apps/desktop/package.json`
- Modify: `pnpm-lock.yaml`
- Create: `apps/desktop/build/entitlements.mac.plist`
- Create: `docs/release/cloud-updates.md`

**Interfaces:**
- Produces: signed macOS DMG/ZIP update artifacts and generic provider metadata
- Consumes: `MATOU_UPDATE_BASE_URL`, `MATOU_UPDATE_CHANNEL`, Apple signing/notarization environment

- [ ] Add `electron-updater` as a runtime dependency.
- [ ] Configure macOS DMG and ZIP targets, hardened runtime, entitlements and generic HTTPS publish metadata.
- [ ] Document artifact upload order, required server headers, signing variables and rollback procedure using exact commands.
- [ ] Run package build with signing discovery disabled and verify `stable-mac.yml`, ZIP, DMG and blockmap files.
- [ ] Commit release configuration.

### Task 5: End-to-end verification

**Files:**
- Modify only files required by failures found during verification.

**Interfaces:**
- Consumes: all preceding tasks
- Produces: releasable updater branch

- [ ] Run `git diff --check`.
- [ ] Run package builds and desktop typecheck.
- [ ] Run all unit tests and record counts.
- [ ] Run a packaged app with a local generic update feed or injected fixture and capture the four approved UI states.
- [ ] Compare the screenshots with the approved Mockup for placement, density, state copy and activity protection.
- [ ] Commit final verification fixes and report the remaining server/signing inputs required for the first real release.
