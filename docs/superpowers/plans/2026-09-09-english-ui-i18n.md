# English UI (i18n) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the desktop UI render in English when the system language is not Chinese (with a tray override), without changing any existing Chinese test assertion.

**Architecture:** A `Locale` contract in `@matou/contracts`; the main process resolves it (env → stored preference → system), exposes it over the preload bridge and passes it to the runtime as `MATOU_LOCALE`. The renderer and the runtime each hold a typed message catalog (`zh-CN` is the source shape, `en` is typed against it) split into per-area namespace files so tasks can run in parallel without touching the same file.

**Tech Stack:** TypeScript 5, React 19, Electron 43, Vitest + @testing-library/react (jsdom), Playwright (`tests/e2e`), pnpm workspaces.

**Spec:** `docs/superpowers/specs/2026-09-09-english-ui-design.md`

## Global Constraints

- Do not write the retired product identifier (see `tooling/project-identifier-policy.mjs`) anywhere; `pnpm check:identifiers` runs in `pnpm test` and fails the build.
- Default locale without any signal is `zh-CN`. Existing tests assert Chinese text and must keep passing untouched.
- `mt-cli.ts`, `productName`, `进入码头.workflow`, `APP_STORAGE_DIRECTORY_NAME`, and the three Chinese regexes used for parsing (see spec) are out of scope.
- English brand name is `Matou`; `Claude Code` is never translated; sentence case for labels; straight quotes in English.
- Every message with a variable is a function `(…) => string`; English count messages pluralise inside the function.
- Comments in code are English (repo convention).
- Run tests from the package directory: `cd apps/desktop && npx vitest run <path>` / `cd apps/runtime && npx vitest run <path>`; workspace-wide: `pnpm typecheck`, `pnpm test`.
- Commit after every task with the trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

---

## File map

| File | Responsibility |
|---|---|
| `packages/contracts/src/locale.ts` (+ `.test.ts`) | `Locale`, `LocalePreference`, `resolveLocale`, `isLocale`, `localeFromSystem` |
| `apps/desktop/src/main/locale-store.ts` (+ `.test.ts`) | Persist preference in `<userData>/locale.json`, resolve current locale, notify listeners |
| `apps/desktop/src/main/messages.ts` | Main-process strings (tray, DAG title, update/recovery errors) per locale |
| `apps/desktop/src/shared/desktop-api.ts` | Bridge types + channels for locale |
| `apps/desktop/src/preload/index.ts` | Bridge implementation |
| `apps/desktop/src/main/index.ts` | Wire store, tray language submenu, DAG title, IPC handlers |
| `apps/desktop/src/main/runtime-host.ts` | Pass `MATOU_LOCALE` to the runtime; localise its 3 error strings |
| `apps/desktop/src/renderer/src/i18n/catalog.ts` | `CatalogShape<T>` type |
| `apps/desktop/src/renderer/src/i18n/current.ts` | Module-level current locale + `messages()` for non-React code |
| `apps/desktop/src/renderer/src/i18n/LocaleProvider.tsx` (+ `.test.tsx`) | React provider, `useLocale`, `useMessages`, `<html lang>` |
| `apps/desktop/src/renderer/src/i18n/messages/index.ts` | Composes namespaces into `Messages`, `MESSAGES` |
| `apps/desktop/src/renderer/src/i18n/messages/<namespace>.ts` | One file per namespace: `<ns>ZhCN` and `<ns>En` |
| `apps/runtime/src/i18n/locale.ts` (+ `.test.ts`) | `runtimeLocale()` from `MATOU_LOCALE` |
| `apps/runtime/src/i18n/messages.ts` + `messages/<namespace>.ts` | Runtime catalog, `runtimeMessages()` |
| `tests/e2e/matou-fixture.ts`, `tests/e2e/locale-english.spec.ts` | Pin `zh-CN`; English smoke test |

---

### Task 1: Locale contract

**Files:**
- Create: `packages/contracts/src/locale.ts`
- Create: `packages/contracts/src/locale.test.ts`
- Modify: `packages/contracts/src/index.ts` (add `export * from './locale'`)

**Interfaces:**
- Produces: `LOCALES`, `Locale`, `LocalePreference`, `DEFAULT_LOCALE`, `isLocale(value: unknown): value is Locale`, `localeFromSystem(system: string | null | undefined): Locale`, `resolveLocale(input: { env?: string | undefined; preference?: LocalePreference | undefined; system?: string | null | undefined }): Locale`.

- [ ] **Step 1: Write the failing test**

```ts
// packages/contracts/src/locale.test.ts
import { describe, expect, it } from 'vitest'

import { DEFAULT_LOCALE, isLocale, localeFromSystem, resolveLocale } from './locale'

describe('locale', () => {
  it('accepts only the supported locales', () => {
    expect(isLocale('zh-CN')).toBe(true)
    expect(isLocale('en')).toBe(true)
    expect(isLocale('en-US')).toBe(false)
    expect(isLocale(undefined)).toBe(false)
  })

  it('maps any Chinese system locale to zh-CN and everything else to en', () => {
    expect(localeFromSystem('zh-CN')).toBe('zh-CN')
    expect(localeFromSystem('zh-Hant-TW')).toBe('zh-CN')
    expect(localeFromSystem('en-US')).toBe('en')
    expect(localeFromSystem('ja')).toBe('en')
    expect(localeFromSystem(undefined)).toBe(DEFAULT_LOCALE)
  })

  it('resolves env, then preference, then system', () => {
    expect(resolveLocale({ env: 'en', preference: 'zh-CN', system: 'zh-CN' })).toBe('en')
    expect(resolveLocale({ env: 'nope', preference: 'en', system: 'zh-CN' })).toBe('en')
    expect(resolveLocale({ preference: 'system', system: 'fr-FR' })).toBe('en')
    expect(resolveLocale({})).toBe('zh-CN')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/contracts && npx vitest run src/locale.test.ts`
Expected: FAIL — cannot find module `./locale`.

- [ ] **Step 3: Write the implementation**

```ts
// packages/contracts/src/locale.ts
export const LOCALES = ['zh-CN', 'en'] as const
export type Locale = (typeof LOCALES)[number]
export type LocalePreference = Locale | 'system'
export const DEFAULT_LOCALE: Locale = 'zh-CN'

export function isLocale(value: unknown): value is Locale {
  return typeof value === 'string' && (LOCALES as readonly string[]).includes(value)
}

export function isLocalePreference(value: unknown): value is LocalePreference {
  return value === 'system' || isLocale(value)
}

/** Any Chinese variant keeps the Chinese UI; every other system language gets English. */
export function localeFromSystem(system: string | null | undefined): Locale {
  if (!system) return DEFAULT_LOCALE
  return system.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en'
}

export function resolveLocale(input: {
  env?: string | undefined
  preference?: LocalePreference | undefined
  system?: string | null | undefined
}): Locale {
  if (isLocale(input.env)) return input.env
  if (isLocale(input.preference)) return input.preference
  return localeFromSystem(input.system)
}
```

Add `export * from './locale'` to `packages/contracts/src/index.ts`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/contracts && npx vitest run src/locale.test.ts && pnpm build`
Expected: 3 tests PASS; build succeeds (later tasks import from the built package).

- [ ] **Step 5: Commit**

```bash
git add packages/contracts/src/locale.ts packages/contracts/src/locale.test.ts packages/contracts/src/index.ts
git commit -m "feat(contracts): locale type and resolution order"
```

---

### Task 2: Main process — locale store, bridge, tray, runtime env

**Files:**
- Create: `apps/desktop/src/main/locale-store.ts`, `apps/desktop/src/main/locale-store.test.ts`
- Create: `apps/desktop/src/main/messages.ts`
- Modify: `apps/desktop/src/shared/desktop-api.ts` (interface + `DESKTOP_CHANNELS`)
- Modify: `apps/desktop/src/preload/index.ts`
- Modify: `apps/desktop/src/main/index.ts` (lines ~305-320 DAG title, ~387-400 startup, ~448-460 tray, IPC handlers near line 469)
- Modify: `apps/desktop/src/main/runtime-host.ts` (constructor, `#launch` env, three error strings at lines 161, 221, 357)
- Modify: `apps/desktop/src/main/app-update-manager.ts` (lines 98, 144, 245), `apps/desktop/src/main/manual-update-downloader.ts` (lines 20, 21, 54)

**Interfaces:**
- Consumes: Task 1 (`Locale`, `LocalePreference`, `resolveLocale`, `isLocalePreference`).
- Produces: `MatouDesktopApi.getLocale(): Promise<Locale>`, `getLocalePreference(): Promise<LocalePreference>`, `setLocalePreference(preference: LocalePreference): Promise<Locale>`, `onLocaleChanged(listener: (locale: Locale) => void): () => void`; channels `matou:get-locale`, `matou:get-locale-preference`, `matou:set-locale-preference`, `matou:locale-changed`; env `MATOU_LOCALE` for the runtime; `mainMessages(locale): MainMessages`.

- [ ] **Step 1: Write the failing store test**

```ts
// apps/desktop/src/main/locale-store.test.ts
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { LocaleStore } from './locale-store'

describe('LocaleStore', () => {
  const roots: string[] = []
  const root = () => { const dir = mkdtempSync(join(tmpdir(), 'matou-locale-')); roots.push(dir); return dir }
  afterEach(() => { for (const dir of roots.splice(0)) rmSync(dir, { recursive: true, force: true }) })

  it('follows the system locale when nothing is stored', () => {
    const store = new LocaleStore({ file: join(root(), 'locale.json'), system: () => 'en-US' })
    expect(store.preference()).toBe('system')
    expect(store.current()).toBe('en')
  })

  it('persists an explicit preference and notifies listeners', () => {
    const file = join(root(), 'locale.json')
    const store = new LocaleStore({ file, system: () => 'en-US' })
    const listener = vi.fn()
    store.onChange(listener)
    expect(store.set('zh-CN')).toBe('zh-CN')
    expect(listener).toHaveBeenCalledWith('zh-CN')
    expect(JSON.parse(readFileSync(file, 'utf8'))).toEqual({ preference: 'zh-CN' })
    expect(new LocaleStore({ file, system: () => 'en-US' }).current()).toBe('zh-CN')
  })

  it('lets the environment win and ignores corrupt files', () => {
    const file = join(root(), 'locale.json')
    const store = new LocaleStore({ file, system: () => 'zh-CN', env: 'en' })
    expect(store.current()).toBe('en')
    const broken = new LocaleStore({ file, system: () => 'zh-CN' })
    require('node:fs').writeFileSync(file, '{not json')
    expect(broken.preference()).toBe('system')
  })
})
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd apps/desktop && npx vitest run src/main/locale-store.test.ts`
Expected: FAIL — cannot find module `./locale-store`.

- [ ] **Step 3: Implement the store and the main catalog**

```ts
// apps/desktop/src/main/locale-store.ts
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'

import { isLocalePreference, resolveLocale, type Locale, type LocalePreference } from '@matou/contracts'

export interface LocaleStoreOptions {
  /** JSON file holding `{ "preference": "system" | "zh-CN" | "en" }`. */
  file: string
  /** Returns the OS locale, e.g. `app.getLocale()`. */
  system: () => string
  /** `process.env.MATOU_LOCALE`; wins over everything when it is a valid locale. */
  env?: string | undefined
}

export class LocaleStore {
  readonly #listeners = new Set<(locale: Locale) => void>()
  constructor(private readonly options: LocaleStoreOptions) {}

  preference(): LocalePreference {
    try {
      const parsed: unknown = JSON.parse(readFileSync(this.options.file, 'utf8'))
      const value = (parsed as { preference?: unknown } | null)?.preference
      return isLocalePreference(value) ? value : 'system'
    } catch {
      return 'system'
    }
  }

  current(): Locale {
    return resolveLocale({ env: this.options.env, preference: this.preference(), system: this.options.system() })
  }

  set(preference: LocalePreference): Locale {
    mkdirSync(dirname(this.options.file), { recursive: true })
    writeFileSync(this.options.file, JSON.stringify({ preference }))
    const locale = this.current()
    for (const listener of this.#listeners) listener(locale)
    return locale
  }

  onChange(listener: (locale: Locale) => void): () => void {
    this.#listeners.add(listener)
    return () => { this.#listeners.delete(listener) }
  }
}
```

```ts
// apps/desktop/src/main/messages.ts
import type { Locale } from '@matou/contracts'

export interface MainMessages {
  showApp: string
  quit: string
  language: string
  followSystem: string
  chinese: string
  english: string
  restartHint: string
  dagWindowTitle: string
  updateDownloaderMissing: string
  updateDmgMissing: string
  updateCheckFailed: string
  updateDownloadHttp: (status: number) => string
  updateEmptyBody: string
  updateChecksumMismatch: string
  recoveryCycleChanged: string
  recoveryExitedDuringOperation: string
  recoveryOperationFailed: string
}

const ZH_CN: MainMessages = {
  showApp: '显示码头',
  quit: '退出',
  language: '语言',
  followSystem: '跟随系统',
  chinese: '中文',
  english: 'English',
  restartHint: '运行时消息在重启后切换',
  dagWindowTitle: '码头 · 会话 DAG',
  updateDownloaderMissing: '应用内下载器尚未初始化',
  updateDmgMissing: '已下载的 DMG 路径不存在',
  updateCheckFailed: '更新检查失败',
  updateDownloadHttp: (status) => `更新文件下载失败（HTTP ${status}）`,
  updateEmptyBody: '更新服务器没有返回文件内容',
  updateChecksumMismatch: '更新文件完整性校验失败',
  recoveryCycleChanged: '数据库恢复周期已更新，本次操作已停止',
  recoveryExitedDuringOperation: '数据库恢复操作未完成：Runtime 在恢复操作期间退出',
  recoveryOperationFailed: '数据库恢复操作失败'
}

const EN: MainMessages = {
  showApp: 'Show Matou',
  quit: 'Quit',
  language: 'Language',
  followSystem: 'Follow system',
  chinese: '中文',
  english: 'English',
  restartHint: 'Runtime messages switch after a restart',
  dagWindowTitle: 'Matou · Session DAG',
  updateDownloaderMissing: 'The in-app downloader has not been initialised',
  updateDmgMissing: 'The downloaded DMG path does not exist',
  updateCheckFailed: 'Update check failed',
  updateDownloadHttp: (status) => `Update download failed (HTTP ${status})`,
  updateEmptyBody: 'The update server returned no file body',
  updateChecksumMismatch: 'Update file integrity check failed',
  recoveryCycleChanged: 'The database recovery cycle changed; this operation was stopped',
  recoveryExitedDuringOperation: 'Database recovery did not finish: the runtime exited during the operation',
  recoveryOperationFailed: 'Database recovery operation failed'
}

export function mainMessages(locale: Locale): MainMessages {
  return locale === 'en' ? EN : ZH_CN
}
```

Wire the three consumers so their Chinese literals come from the catalog. `RuntimeHost`, `AppUpdateManager` and the manual downloader are constructed in `main/index.ts`; give each a `messages: () => MainMessages` option (a getter so a locale change applies without reconstructing) and replace the literals: `runtime-host.ts:161` → `this.#messages().recoveryCycleChanged`, `:221` → `.recoveryExitedDuringOperation`, `:357` → `.recoveryOperationFailed`; `app-update-manager.ts:98` → `updateDownloaderMissing`, `:144` → `updateDmgMissing`, `:245` → `updateCheckFailed`; `manual-update-downloader.ts:20/21/54` → `updateDownloadHttp(response.status)`, `updateEmptyBody`, `updateChecksumMismatch`. Default the option to `() => mainMessages('zh-CN')` in each constructor so existing unit tests (`runtime-host.test.ts:363/369`, `manual-update-downloader.test.ts:48`) keep their Chinese expectations.

- [ ] **Step 4: Extend the bridge**

In `apps/desktop/src/shared/desktop-api.ts` add to `MatouDesktopApi`:

```ts
  getLocale(): Promise<Locale>
  getLocalePreference(): Promise<LocalePreference>
  setLocalePreference(preference: LocalePreference): Promise<Locale>
  onLocaleChanged(listener: (locale: Locale) => void): () => void
```

(import `Locale`, `LocalePreference` from `@matou/contracts`) and to `DESKTOP_CHANNELS`:

```ts
  getLocale: 'matou:get-locale',
  getLocalePreference: 'matou:get-locale-preference',
  setLocalePreference: 'matou:set-locale-preference',
  localeChanged: 'matou:locale-changed'
```

In `apps/desktop/src/preload/index.ts` add to `desktopApi`:

```ts
  getLocale: () => ipcRenderer.invoke(DESKTOP_CHANNELS.getLocale),
  getLocalePreference: () => ipcRenderer.invoke(DESKTOP_CHANNELS.getLocalePreference),
  setLocalePreference: (preference) => ipcRenderer.invoke(DESKTOP_CHANNELS.setLocalePreference, preference),
  onLocaleChanged: (listener) => {
    const handler = (_event: Electron.IpcRendererEvent, locale: Locale) => listener(locale)
    ipcRenderer.on(DESKTOP_CHANNELS.localeChanged, handler)
    return () => ipcRenderer.removeListener(DESKTOP_CHANNELS.localeChanged, handler)
  },
```

- [ ] **Step 5: Wire main/index.ts**

Right after the `userData` path is set (line ~92):

```ts
const localeStore = new LocaleStore({
  file: join(app.getPath('userData'), 'locale.json'),
  system: () => app.getLocale(),
  env: process.env.MATOU_LOCALE
})
const messages = () => mainMessages(localeStore.current())
```

- `RuntimeHost` (line ~394): pass `{ messages }` and, in `runtime-host.ts` `#launch`, add `MATOU_LOCALE: this.#locale()` to the fork `env` where `#locale` is a new constructor option `locale: () => Locale` (default `() => 'zh-CN'`). In `index.ts` pass `locale: () => localeStore.current()`.
- DAG window title (line ~313): `title: messages().dagWindowTitle`.
- Tray: extract a `buildTrayMenu()` function and call it at creation and from `localeStore.onChange`:

```ts
function buildTrayMenu(): Menu {
  const m = messages()
  const preference = localeStore.preference()
  const item = (label: string, value: LocalePreference): MenuItemConstructorOptions => ({
    label, type: 'radio', checked: preference === value,
    click: () => { localeStore.set(value) }
  })
  return Menu.buildFromTemplate([
    { label: m.showApp, click: () => { const id = windows.firstLiveWindowId(); if (id) windows.showWindow(id); else void createWindow() } },
    { type: 'separator' },
    { label: m.language, submenu: [
      item(m.followSystem, 'system'), item(m.chinese, 'zh-CN'), item(m.english, 'en'),
      { type: 'separator' }, { label: m.restartHint, enabled: false }
    ] },
    { type: 'separator' },
    { label: m.quit, click: () => { quitting = true; app.quit() } }
  ])
}
```

```ts
localeStore.onChange((locale) => {
  tray?.setContextMenu(buildTrayMenu())
  for (const window of BrowserWindow.getAllWindows()) {
    if (!window.isDestroyed()) window.webContents.send(DESKTOP_CHANNELS.localeChanged, locale)
  }
})
```

- IPC handlers next to the existing ones (line ~469):

```ts
ipcMain.handle(DESKTOP_CHANNELS.getLocale, () => localeStore.current())
ipcMain.handle(DESKTOP_CHANNELS.getLocalePreference, () => localeStore.preference())
ipcMain.handle(DESKTOP_CHANNELS.setLocalePreference, (_event, preference: LocalePreference) => {
  if (!isLocalePreference(preference)) throw new Error('Invalid locale preference')
  return localeStore.set(preference)
})
```

- [ ] **Step 6: Run main tests and typecheck**

Run: `cd apps/desktop && npx vitest run src/main && pnpm typecheck`
Expected: all main tests PASS (Chinese defaults unchanged), typecheck clean.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/main apps/desktop/src/shared/desktop-api.ts apps/desktop/src/preload/index.ts
git commit -m "feat(desktop): resolve locale in main, expose over bridge, tray language menu"
```

---

### Task 3: Renderer i18n infrastructure and namespace skeleton

**Files:**
- Create: `apps/desktop/src/renderer/src/i18n/catalog.ts`
- Create: `apps/desktop/src/renderer/src/i18n/current.ts`
- Create: `apps/desktop/src/renderer/src/i18n/LocaleProvider.tsx`, `LocaleProvider.test.tsx`
- Create: `apps/desktop/src/renderer/src/i18n/messages/index.ts`
- Create: `apps/desktop/src/renderer/src/i18n/messages/{common,app,hierarchyShell,hierarchyTerminal,hud,sessionCanvas,updates,recovery,dag,notifications,terminal}.ts`
- Modify: `apps/desktop/src/renderer/src/main.tsx` (wrap in `LocaleProvider`)
- Modify: `apps/desktop/src/renderer/src/hierarchy/EmptyWorkspaceState.tsx` (exemplar conversion)
- Modify: `apps/desktop/src/renderer/index.html` (`lang` is set at runtime; leave `zh-CN` as the static default)

**Interfaces:**
- Consumes: Task 1 types; Task 2 bridge methods (optional at runtime: tests have no bridge).
- Produces: `CatalogShape<T>`; `currentLocale(): Locale`; `setCurrentLocale(locale)`; `messages(): Messages`; `LocaleProvider({ children, initialLocale? })`; `useLocale(): Locale`; `useMessages(): Messages`; `MESSAGES: Record<Locale, Messages>`; one exported pair per namespace file: `<ns>ZhCN` and `<ns>En: CatalogShape<typeof <ns>ZhCN>`; `common` keys `cancel`, `confirm`, `close`, `retry`, `ok`, `loading`, `unknown`.

- [ ] **Step 1: Write the failing provider test**

```tsx
// apps/desktop/src/renderer/src/i18n/LocaleProvider.test.tsx
// @vitest-environment jsdom
import { act, cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { EmptyWorkspaceState } from '../hierarchy/EmptyWorkspaceState'
import { LocaleProvider } from './LocaleProvider'
import { currentLocale, messages } from './current'

describe('LocaleProvider', () => {
  afterEach(() => { cleanup(); Reflect.deleteProperty(window, 'matouDesktop'); document.documentElement.lang = 'zh-CN' })

  it('defaults to Chinese without a desktop bridge', () => {
    render(<LocaleProvider><EmptyWorkspaceState onCreate={() => {}} /></LocaleProvider>)
    expect(screen.getByRole('heading', { name: '还没有工作区' })).toBeTruthy()
    expect(currentLocale()).toBe('zh-CN')
  })

  it('renders English when the bridge reports en and follows later changes', async () => {
    let emit: (locale: 'zh-CN' | 'en') => void = () => {}
    Object.assign(window, { matouDesktop: {
      getLocale: vi.fn(async () => 'en'),
      onLocaleChanged: vi.fn((listener: (locale: 'zh-CN' | 'en') => void) => { emit = listener; return () => {} })
    } })
    render(<LocaleProvider><EmptyWorkspaceState onCreate={() => {}} /></LocaleProvider>)
    expect(await screen.findByRole('heading', { name: 'No workspace yet' })).toBeTruthy()
    expect(document.documentElement.lang).toBe('en')
    expect(messages().common.cancel).toBe('Cancel')
    act(() => emit('zh-CN'))
    expect(screen.getByRole('heading', { name: '还没有工作区' })).toBeTruthy()
  })

  it('honours initialLocale for tests that render a single locale', () => {
    render(<LocaleProvider initialLocale="en"><EmptyWorkspaceState onCreate={() => {}} /></LocaleProvider>)
    expect(screen.getByRole('button', { name: 'New workspace' })).toBeTruthy()
  })
})
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd apps/desktop && npx vitest run src/renderer/src/i18n`
Expected: FAIL — modules missing.

- [ ] **Step 3: Implement the infrastructure**

```ts
// apps/desktop/src/renderer/src/i18n/catalog.ts
/** English catalogs are typed against the Chinese shape: a missing key is a compile error. */
export type CatalogShape<T> = {
  [K in keyof T]: T[K] extends (...args: infer A) => string
    ? (...args: A) => string
    : T[K] extends string ? string : CatalogShape<T[K]>
}
```

```ts
// apps/desktop/src/renderer/src/i18n/messages/common.ts
export const commonZhCN = {
  cancel: '取消',
  confirm: '确认',
  close: '关闭',
  retry: '重试',
  ok: '好',
  loading: '加载中',
  unknown: '未知'
}
export const commonEn: import('../catalog').CatalogShape<typeof commonZhCN> = {
  cancel: 'Cancel',
  confirm: 'Confirm',
  close: 'Close',
  retry: 'Retry',
  ok: 'OK',
  loading: 'Loading',
  unknown: 'Unknown'
}
```

Create the other ten namespace files with the same two exports and empty objects, e.g.

```ts
// apps/desktop/src/renderer/src/i18n/messages/hud.ts
import type { CatalogShape } from '../catalog'
export const hudZhCN = {}
export const hudEn: CatalogShape<typeof hudZhCN> = {}
```

```ts
// apps/desktop/src/renderer/src/i18n/messages/index.ts
import type { Locale } from '@matou/contracts'

import type { CatalogShape } from '../catalog'
import { appEn, appZhCN } from './app'
import { commonEn, commonZhCN } from './common'
import { dagEn, dagZhCN } from './dag'
import { hierarchyShellEn, hierarchyShellZhCN } from './hierarchyShell'
import { hierarchyTerminalEn, hierarchyTerminalZhCN } from './hierarchyTerminal'
import { hudEn, hudZhCN } from './hud'
import { notificationsEn, notificationsZhCN } from './notifications'
import { recoveryEn, recoveryZhCN } from './recovery'
import { sessionCanvasEn, sessionCanvasZhCN } from './sessionCanvas'
import { terminalEn, terminalZhCN } from './terminal'
import { updatesEn, updatesZhCN } from './updates'

export const zhCN = {
  common: commonZhCN, app: appZhCN, hierarchyShell: hierarchyShellZhCN, hierarchyTerminal: hierarchyTerminalZhCN,
  hud: hudZhCN, sessionCanvas: sessionCanvasZhCN, updates: updatesZhCN, recovery: recoveryZhCN,
  dag: dagZhCN, notifications: notificationsZhCN, terminal: terminalZhCN
}
export type Messages = CatalogShape<typeof zhCN>
export const en: Messages = {
  common: commonEn, app: appEn, hierarchyShell: hierarchyShellEn, hierarchyTerminal: hierarchyTerminalEn,
  hud: hudEn, sessionCanvas: sessionCanvasEn, updates: updatesEn, recovery: recoveryEn,
  dag: dagEn, notifications: notificationsEn, terminal: terminalEn
}
export const MESSAGES: Record<Locale, Messages> = { 'zh-CN': zhCN, en }
```

```ts
// apps/desktop/src/renderer/src/i18n/current.ts
import { DEFAULT_LOCALE, type Locale } from '@matou/contracts'

import { MESSAGES, type Messages } from './messages'

let locale: Locale = DEFAULT_LOCALE
const listeners = new Set<(locale: Locale) => void>()

export function currentLocale(): Locale { return locale }
export function setCurrentLocale(next: Locale): void {
  if (next === locale) return
  locale = next
  for (const listener of listeners) listener(next)
}
export function onCurrentLocaleChange(listener: (locale: Locale) => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}
/** For code outside React (command builders, notification text). Components use `useMessages()`. */
export function messages(): Messages { return MESSAGES[locale] }
```

```tsx
// apps/desktop/src/renderer/src/i18n/LocaleProvider.tsx
import type { Locale } from '@matou/contracts'
import { createContext, useContext, useEffect, useState, type ReactNode } from 'react'

import { currentLocale, setCurrentLocale } from './current'
import { MESSAGES, type Messages } from './messages'

const LocaleContext = createContext<Locale>(currentLocale())

export function LocaleProvider({ children, initialLocale }: { children: ReactNode; initialLocale?: Locale }) {
  const [locale, setLocale] = useState<Locale>(() => {
    if (initialLocale) setCurrentLocale(initialLocale)
    return currentLocale()
  })
  useEffect(() => {
    const bridge = window.matouDesktop as Partial<typeof window.matouDesktop> | undefined
    if (!bridge?.getLocale || !bridge.onLocaleChanged) return
    let active = true
    const apply = (next: Locale) => { if (!active) return; setCurrentLocale(next); setLocale(next) }
    void bridge.getLocale().then(apply)
    const unsubscribe = bridge.onLocaleChanged(apply)
    return () => { active = false; unsubscribe() }
  }, [])
  useEffect(() => { document.documentElement.lang = locale }, [locale])
  return <LocaleContext.Provider value={locale}>{children}</LocaleContext.Provider>
}

export function useLocale(): Locale { return useContext(LocaleContext) }
export function useMessages(): Messages { return MESSAGES[useLocale()] }
```

Exemplar conversion:

```tsx
// apps/desktop/src/renderer/src/hierarchy/EmptyWorkspaceState.tsx
import { useMessages } from '../i18n/LocaleProvider'

export function EmptyWorkspaceState({ onCreate }: { onCreate(): void }) {
  const m = useMessages().hierarchyShell.emptyWorkspace
  return <section><h2>{m.title}</h2><p>{m.hint}</p><button onClick={onCreate}>{m.create}</button></section>
}
```

with, in `messages/hierarchyShell.ts`:

```ts
export const hierarchyShellZhCN = {
  emptyWorkspace: { title: '还没有工作区', hint: '选择一个本地目录开始工作。', create: '新建工作区' }
}
export const hierarchyShellEn: CatalogShape<typeof hierarchyShellZhCN> = {
  emptyWorkspace: { title: 'No workspace yet', hint: 'Pick a local directory to start working.', create: 'New workspace' }
}
```

`main.tsx`: `createRoot(root).render(<LocaleProvider><RuntimeProvider><App /></RuntimeProvider></LocaleProvider>)`.

- [ ] **Step 4: Run tests and typecheck**

Run: `cd apps/desktop && npx vitest run src/renderer/src/i18n src/renderer/src/hierarchy && pnpm typecheck`
Expected: PASS, including every existing hierarchy test.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/renderer/src/i18n apps/desktop/src/renderer/src/main.tsx apps/desktop/src/renderer/src/hierarchy/EmptyWorkspaceState.tsx
git commit -m "feat(renderer): locale provider and typed message catalog skeleton"
```

---

### Task 4: Pin the test locale

**Files:**
- Modify: `apps/runtime/vitest.config.ts`, `apps/desktop/vitest.config.ts` (add `env: { MATOU_LOCALE: 'zh-CN' }` under `test`)
- Modify: `tests/e2e/matou-fixture.ts` (`startMatou` env: add `MATOU_LOCALE: 'zh-CN'` **before** `...options.env` so a spec can override)
- Modify: `tests/e2e/packaged-runtime.spec.ts:100` and `tests/e2e/terminal-channel.spec.ts:14,104,174` (`electron.launch` env: add `MATOU_LOCALE: 'zh-CN'`)

**Interfaces:**
- Consumes: Task 2 (main honours `MATOU_LOCALE`).
- Produces: every existing suite runs in `zh-CN` regardless of the developer's OS language.

- [ ] **Step 1: Apply the four edits**

```ts
// apps/runtime/vitest.config.ts and apps/desktop/vitest.config.ts — inside `test: { … }`
    env: { MATOU_LOCALE: 'zh-CN' },
```

```ts
// tests/e2e/matou-fixture.ts — inside electron.launch({ env: { … } })
      ...process.env,
      MATOU_LOCALE: 'zh-CN',
      ...options.env,
```

- [ ] **Step 2: Verify**

Run: `pnpm typecheck && cd apps/desktop && npx vitest run src/main/locale-store.test.ts`
Expected: PASS (the store test passes explicit `env`, so the pinned variable does not affect it).

Run: `pnpm build && npx playwright test tests/e2e/prd-05-hierarchy.spec.ts --reporter=line`
Expected: PASS with Chinese assertions on a machine whose OS language may be English.

- [ ] **Step 3: Commit**

```bash
git add apps/runtime/vitest.config.ts apps/desktop/vitest.config.ts tests/e2e/matou-fixture.ts tests/e2e/packaged-runtime.spec.ts tests/e2e/terminal-channel.spec.ts
git commit -m "test: pin MATOU_LOCALE=zh-CN for unit and e2e suites"
```

---

## Renderer namespace tasks (5–10): shared procedure

Each task below converts one area. The procedure is identical; only the file list, namespace and examples differ.

1. Read every listed file and collect each Chinese user-visible string (JSX text, `aria-label`, `title`, `placeholder`, alert/confirm bodies, label helper functions, `Record` maps, module-level constants). Skip test files and anything listed as out of scope in the spec.
2. Build `<ns>ZhCN` in `messages/<ns>.ts`, grouped by component in camelCase (`taskSidebar.newTask`). Static text → string. Text with variables → function with typed parameters. Text chosen by a `switch` over a union → nested object keyed by the union values (`forkStage: { pending: '…', starting: '…' }`).
3. Write `<ns>En` with the same shape. Pluralise in the function: `(n: number) => n === 1 ? '1 child session' : \`${n} child sessions\``.
4. Replace usages. Components: `const m = useMessages().<ns>` (call `useMessages()` at the top of the component, never conditionally). Non-component modules and callbacks created outside render: `messages().<ns>` from `../i18n/current`. `aria-label`/`title` that duplicated the visible text reuse the same key.
5. Replace hardcoded `'zh-CN'` in `Intl.DateTimeFormat` / `toLocaleString` with `useLocale()` (components) or `currentLocale()` (modules).
6. Run the area's existing tests: `cd apps/desktop && npx vitest run src/renderer/src/<area>`. They must pass unchanged.
7. Add one English test file `<Component>.locale.test.tsx` rendering the most important component inside `<LocaleProvider initialLocale="en">` and asserting two English accessible names.
8. `pnpm typecheck`, then commit.

Naming and style rules are in the spec ("English copy rules"). When a string embeds CJK quotes around user data, the English version uses straight double quotes.

### Task 5: Namespace `hierarchyShell`

**Files:**
- Modify: `apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/{TaskSidebar,HierarchyShell,SceneTabBar,WorkspaceSwitcher,WorkspaceKanbanBoard,RenameDialog,ConfirmDialog,ShortcutPanel,ModelSwitchSettings}.tsx`, `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`
- Create: `apps/desktop/src/renderer/src/hierarchy/TaskSidebar.locale.test.tsx`

**Interfaces:**
- Consumes: Task 3.
- Produces: in `en`, the TaskSidebar create-task button has accessible name `New task`, the workspace switcher trigger has accessible name `Switch workspace`, kanban columns are `Ready`, `Active`, `Blocked`, `Done` (used by Task 13's e2e smoke test). `WORKSPACE_PATH_MESSAGE` duplicated in `SceneTabBar.tsx:266` and `TaskSidebar.tsx:386` becomes one key `workspacePathUnavailable`.

- [ ] **Step 1: Extract strings to `hierarchyShellZhCN`** (keep the `emptyWorkspace` group from Task 3). Example of a map conversion for `WorkspaceKanbanBoard.tsx:7`:

```ts
// before
const COLUMNS = [{ status: 'ready', label: '待办' }, …]
// after (messages/hierarchyShell.ts)
kanban: { ready: '待办', active: '进行中', blocked: '阻塞', done: '完成' }
// after (component)
const m = useMessages().hierarchyShell.kanban
const columns = (['ready', 'active', 'blocked', 'done'] as const).map((status) => ({ status, label: m[status] }))
```

Example of a three-variant sentence (`ModelSwitchSettings.tsx`):

```ts
switched: (provider: string, updated: number, deferred: number) =>
  `已切换为 ${provider}；${updated} 个 Claude Code 会话已更新，${deferred} 个会话暂缓并保持原配置`
// en
switched: (provider, updated, deferred) =>
  `Switched to ${provider}; ${updated} Claude Code ${updated === 1 ? 'session' : 'sessions'} updated, ${deferred} deferred with the previous configuration`
```

- [ ] **Step 2: Replace usages; run `cd apps/desktop && npx vitest run src/renderer/src/hierarchy`** — expected PASS unchanged.

- [ ] **Step 3: Write the English test**

```tsx
// apps/desktop/src/renderer/src/hierarchy/TaskSidebar.locale.test.tsx
// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import { LocaleProvider } from '../i18n/LocaleProvider'
import { TaskSidebar } from './TaskSidebar'
// reuse the `fixture()` and `commands()` helpers from hierarchy-components.test.tsx by extracting them to
// hierarchy-test-fixtures.ts if they are not exported yet
import { commands, fixture } from './hierarchy-test-fixtures'

describe('TaskSidebar in English', () => {
  afterEach(cleanup)
  it('labels the create button and workspace switcher', () => {
    render(<LocaleProvider initialLocale="en"><TaskSidebar projection={fixture()} commands={commands()} /></LocaleProvider>)
    expect(screen.getByRole('button', { name: 'New task' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Switch workspace' })).toBeTruthy()
  })
})
```

- [ ] **Step 4: `pnpm typecheck`; commit**

```bash
git add apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts apps/desktop/src/renderer/src/hierarchy
git commit -m "feat(renderer): localise workspace, task and scene chrome"
```

### Task 6: Namespace `hierarchyTerminal`

**Files:**
- Modify: `apps/desktop/src/renderer/src/i18n/messages/hierarchyTerminal.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/{TerminalPane,TerminalSearchBar,StorageFaultOverlay,SessionRecoveryWater,AgentTeamMemberSummary,DetachedTerminalApp,DetachedPlaceholder}.tsx`, `apps/desktop/src/renderer/src/hierarchy/{terminal-close-flow,terminal-loading-state}.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/terminal-loading-state.test.ts` (label → labelKey)
- Create: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.locale.test.tsx`

**Interfaces:**
- Consumes: Task 3.
- Produces: `TerminalLoadingPresentation.labelKey: 'loading' | 'recovering'` (replaces `label`); `hierarchyTerminal.loading: { loading: '加载中', recovering: '恢复中' }`.

- [ ] **Step 1: Change the type-level blocker first**

```ts
// terminal-loading-state.ts
export interface TerminalLoadingPresentation {
  phase: 'loading' | 'recovery'
  labelKey: 'loading' | 'recovering'
}
// … return { phase: 'loading', labelKey: 'loading' } / { phase: 'recovery', labelKey: 'recovering' }
```

Update `terminal-loading-state.test.ts` expectations from `label: '加载中'` to `labelKey: 'loading'` (and `'恢复中'` → `'recovering'`). In `TerminalPane.tsx` render `m.loading[presentation.labelKey]`.

- [ ] **Step 2: Extract the rest.** `terminal-close-flow.ts` builds confirm bodies outside React: use `messages().hierarchyTerminal.closeFlow.runningSessions(runningCount)` etc. `READ_ONLY_REASON` (`TerminalPane.tsx:643`) and `READ_ONLY_RECOVERY_REASON` (`hierarchy-commands.ts:191`, converted in Task 5) share the text: keep Task 5's `hierarchyShell.readOnlyRecoveryReason` and import it from there instead of duplicating.

- [ ] **Step 3: Run `cd apps/desktop && npx vitest run src/renderer/src/hierarchy`** — PASS.

- [ ] **Step 4: English test** rendering `TerminalPane` in `en` (reuse the smallest existing `TerminalPane.test.tsx` setup) asserting the loading label `Loading` and the search bar placeholder `Search terminal`.

- [ ] **Step 5: `pnpm typecheck`; commit** `feat(renderer): localise terminal pane, close flow and recovery overlays`.

### Task 7: Namespace `hud`

**Files:**
- Modify: `apps/desktop/src/renderer/src/i18n/messages/hud.ts`
- Modify: `apps/desktop/src/renderer/src/hud/{TerminalHud,GitControlMenu,EnvironmentControlMenu}.tsx`
- Create: `apps/desktop/src/renderer/src/hud/TerminalHud.locale.test.tsx`

**Interfaces:**
- Consumes: Task 3.
- Produces: `hud.permission`, `hud.model`, `hud.gitTitle`, `hud.environmentState`, `hud.locateReason` nested objects keyed by the existing union values; `GitControlMenu` file counts formatted with `toLocaleString(currentLocale())`.

- [ ] **Step 1: Extract.** `TerminalHud.tsx:410` `permissionLabel`/`modelLabel`/`gitTitle` become nested objects; `TerminalHud.tsx:108-109` identical `title`/`aria-label` share one key. `EnvironmentControlMenu.tsx:128` `locateReasonText` (8 keys) becomes `hud.locateReason`. Multi-line confirm bodies at `TerminalHud.tsx:440-443` keep `\n\n` inside the function result.

- [ ] **Step 2: `cd apps/desktop && npx vitest run src/renderer/src/hud`** — PASS.

- [ ] **Step 3: English test** asserting the HUD's git control accessible name `Git` and the environment menu trigger `Environment` render in `en`.

- [ ] **Step 4: `pnpm typecheck`; commit** `feat(renderer): localise HUD, git and environment menus`.

### Task 8: Namespace `sessionCanvas`

**Files:**
- Modify: `apps/desktop/src/renderer/src/i18n/messages/sessionCanvas.ts`
- Modify: `apps/desktop/src/renderer/src/session-canvas/{SessionLoaderDialog,BranchDialog,RemoveNodeDialog,ForkProgressOverlay,ChildSessionBadge,SessionCarousel,ParentProjection,SessionBreadcrumb,SessionCard,StoppedSessionCard,SessionCanvas}.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/RemoveNodeDialog.locale.test.tsx`

**Interfaces:**
- Consumes: Task 3.
- Produces: `sessionCanvas.forkStage: { pending, starting, succeeded, failed }`; count messages `descendantsRemoved(n)`, `removeSessions(n)`, `running(n)`, `needsInput(n)`, `ownedWorktrees(n)`, `viewChildren(n)`, `sessionsOf(shown, total)`, `matches(n)`, `entries(n)`.

- [ ] **Step 1: Extract.** Example:

```ts
// zh-CN
descendantsRemoved: (n: number) => `当前节点与 ${n} 个后代会话会全部移除。`
// en
descendantsRemoved: (n) => n === 1
  ? 'This node and its 1 descendant session will be removed.'
  : `This node and its ${n} descendant sessions will be removed.`
```

`ForkProgressOverlay.tsx:28 stageLabel(stage)` → `m.forkStage[stage]`.

- [ ] **Step 2: `cd apps/desktop && npx vitest run src/renderer/src/session-canvas`** — PASS.

- [ ] **Step 3: English test** rendering `RemoveNodeDialog` with 3 descendants in `en`, asserting the sentence `This node and its 3 descendant sessions will be removed.` and the confirm button `Remove`.

- [ ] **Step 4: `pnpm typecheck`; commit** `feat(renderer): localise session canvas dialogs and cards`.

### Task 9: Namespaces `updates`, `recovery`, `app`

**Files:**
- Modify: `apps/desktop/src/renderer/src/i18n/messages/{updates,recovery,app}.ts`
- Modify: `apps/desktop/src/renderer/src/updates/AppUpdateControl.tsx`, `apps/desktop/src/renderer/src/recovery/{DatabaseRecoveryPage,ReadOnlyRecoveryBanner}.tsx`, `apps/desktop/src/renderer/src/App.tsx`
- Create: `apps/desktop/src/renderer/src/updates/AppUpdateControl.locale.test.tsx`

**Interfaces:**
- Consumes: Task 3.
- Produces: `updates.retrying(attempt, max)`, `updates.activeSessions(n)`, date formatting via `useLocale()` at `AppUpdateControl.tsx:222` and `DatabaseRecoveryPage.tsx:192`; `app.updateRequired` for `App.tsx:86` (`需要更新 Matou` → `Matou needs an update`).

- [ ] **Step 1: Extract; replace `new Intl.DateTimeFormat('zh-CN', …)` with `new Intl.DateTimeFormat(locale, …)` where `const locale = useLocale()`.**
- [ ] **Step 2: `cd apps/desktop && npx vitest run src/renderer/src/updates src/renderer/src/recovery src/renderer/src/App.test.tsx`** — PASS.
- [ ] **Step 3: English test** for `AppUpdateControl` asserting `Check for updates` and `Install and restart` accessible names in `en`.
- [ ] **Step 4: `pnpm typecheck`; commit** `feat(renderer): localise updates, recovery and app shell`.

### Task 10: Namespaces `dag`, `notifications`, `terminal`

**Files:**
- Modify: `apps/desktop/src/renderer/src/i18n/messages/{dag,notifications,terminal}.ts`
- Modify: `apps/desktop/src/renderer/src/dag/{DagCanvas,DagWindowApp,DagSearch}.tsx`, `apps/desktop/src/renderer/src/notifications/{NotificationCenter.tsx,osc-notification.ts,notification-sound.ts}`, `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Create: `apps/desktop/src/renderer/src/dag/DagSearch.locale.test.tsx`

**Interfaces:**
- Consumes: Task 3.
- Produces: `terminal.historyTrimmed` (text written into xterm at `TerminalSurface.tsx:680`, resolved at write time with `messages()`), `terminal.historyGaps(n)`, `notifications.fallbackBody` (`osc-notification.ts` `'终端通知'`), `dag.search.placeholder`.

- [ ] **Step 1: Extract.** `notification-sound.ts` contains the only Chinese console log; translate it to English unconditionally (logs are not UI).
- [ ] **Step 2: `cd apps/desktop && npx vitest run src/renderer/src/dag src/renderer/src/notifications src/renderer/src/terminal`** — PASS.
- [ ] **Step 3: English test** for `DagSearch` asserting the search box accessible name `Search sessions` in `en`.
- [ ] **Step 4: `pnpm typecheck`; commit** `feat(renderer): localise DAG window, notification center and terminal surface`.

---

### Task 11: Runtime i18n infrastructure + `server` and `control` namespaces

**Files:**
- Create: `apps/runtime/src/i18n/locale.ts`, `apps/runtime/src/i18n/locale.test.ts`
- Create: `apps/runtime/src/i18n/catalog.ts` (same `CatalogShape` as the renderer; runtime cannot import renderer code)
- Create: `apps/runtime/src/i18n/messages.ts`, `apps/runtime/src/i18n/messages/{control,server,sessionCanvas,providerConfig,hierarchy,storage,git,session,shellHistory}.ts` (skeletons; fill `control` and `server` here)
- Modify: `apps/runtime/src/runtime-server.ts`, `apps/runtime/src/index.ts`, `apps/runtime/src/control/{host-navigation-broker,runtime-host-action-facade,host-action-target-resolver,host-topology-projector,fork-batch-coordinator,provider-ready-registry,host-action-confirmation-service}.ts` (and the other three files under `control/` with Chinese strings)

**Interfaces:**
- Consumes: Task 1.
- Produces: `runtimeLocale(): Locale`, `resolveRuntimeLocale(value: string | undefined): Locale`, `resetRuntimeLocaleForTests()`, `runtimeMessages(): RuntimeMessages`, namespace exports `<ns>ZhCN` / `<ns>En`.

- [ ] **Step 1: Write the failing test**

```ts
// apps/runtime/src/i18n/locale.test.ts
import { afterEach, describe, expect, it } from 'vitest'

import { resetRuntimeLocaleForTests, resolveRuntimeLocale, runtimeLocale } from './locale'
import { runtimeMessages } from './messages'

describe('runtime locale', () => {
  const original = process.env.MATOU_LOCALE
  afterEach(() => { process.env.MATOU_LOCALE = original; resetRuntimeLocaleForTests() })

  it('defaults to zh-CN and accepts only known locales', () => {
    expect(resolveRuntimeLocale(undefined)).toBe('zh-CN')
    expect(resolveRuntimeLocale('en')).toBe('en')
    expect(resolveRuntimeLocale('fr')).toBe('zh-CN')
  })

  it('reads MATOU_LOCALE once', () => {
    process.env.MATOU_LOCALE = 'en'
    resetRuntimeLocaleForTests()
    expect(runtimeLocale()).toBe('en')
    expect(runtimeMessages().server.branchReady).toBe('Branch ready')
  })
})
```

- [ ] **Step 2: Run** `cd apps/runtime && npx vitest run src/i18n` — FAIL (modules missing).

- [ ] **Step 3: Implement**

```ts
// apps/runtime/src/i18n/locale.ts
import { DEFAULT_LOCALE, isLocale, type Locale } from '@matou/contracts'

let resolved: Locale | undefined

export function resolveRuntimeLocale(value: string | undefined): Locale {
  return isLocale(value) ? value : DEFAULT_LOCALE
}
/** Read once at process start; the main process passes MATOU_LOCALE when it forks the runtime. */
export function runtimeLocale(): Locale {
  resolved ??= resolveRuntimeLocale(process.env.MATOU_LOCALE)
  return resolved
}
export function resetRuntimeLocaleForTests(): void { resolved = undefined }
```

```ts
// apps/runtime/src/i18n/messages.ts
import type { Locale } from '@matou/contracts'

import type { CatalogShape } from './catalog'
import { runtimeLocale } from './locale'
import { controlEn, controlZhCN } from './messages/control'
import { gitEn, gitZhCN } from './messages/git'
import { hierarchyEn, hierarchyZhCN } from './messages/hierarchy'
import { providerConfigEn, providerConfigZhCN } from './messages/providerConfig'
import { serverEn, serverZhCN } from './messages/server'
import { sessionEn, sessionZhCN } from './messages/session'
import { sessionCanvasEn, sessionCanvasZhCN } from './messages/sessionCanvas'
import { shellHistoryEn, shellHistoryZhCN } from './messages/shellHistory'
import { storageEn, storageZhCN } from './messages/storage'

export const zhCN = {
  control: controlZhCN, server: serverZhCN, sessionCanvas: sessionCanvasZhCN, providerConfig: providerConfigZhCN,
  hierarchy: hierarchyZhCN, storage: storageZhCN, git: gitZhCN, session: sessionZhCN, shellHistory: shellHistoryZhCN
}
export type RuntimeMessages = CatalogShape<typeof zhCN>
export const en: RuntimeMessages = {
  control: controlEn, server: serverEn, sessionCanvas: sessionCanvasEn, providerConfig: providerConfigEn,
  hierarchy: hierarchyEn, storage: storageEn, git: gitEn, session: sessionEn, shellHistory: shellHistoryEn
}
const MESSAGES: Record<Locale, RuntimeMessages> = { 'zh-CN': zhCN, en }
export function runtimeMessages(): RuntimeMessages { return MESSAGES[runtimeLocale()] }
```

`server` must contain at least (from `index.ts:558-561`): `branchReady: '分支已就绪'`, `branchFailed: '分支创建失败'`, `branchReadyBody: '新的分支会话已经可以继续工作'`, `branchIncomplete: '分支创建未完成'` (English: `Branch ready`, `Branch creation failed`, `The new branch session is ready to continue`, `Branch creation did not finish`). Replace the seven module-level `*_MESSAGE` constants in `host-navigation-broker.ts` with getters that read `runtimeMessages().control.<key>` at call time (module-level constants would freeze the locale before the env is read in tests).

- [ ] **Step 4: Run** `cd apps/runtime && npx vitest run` — all runtime tests PASS (pinned `zh-CN`).
- [ ] **Step 5: `pnpm typecheck`; commit** `feat(runtime): locale from MATOU_LOCALE, localise control and server messages`.

### Task 12: Runtime remaining namespaces and default entity names

**Files:**
- Modify: `apps/runtime/src/i18n/messages/{sessionCanvas,providerConfig,hierarchy,storage,git,session,shellHistory}.ts`
- Modify: `apps/runtime/src/session-canvas/{fork-workflow-service,session-canvas-service,provider-mode-service,branch-name}.ts` (+ the other two files in that directory with Chinese strings), `apps/runtime/src/provider-config/provider-config-store.ts`, `apps/runtime/src/hierarchy/*.ts` (2 files), `apps/runtime/src/storage/*.ts` (3 files), `apps/runtime/src/git/git-workspace-service.ts`, `apps/runtime/src/session/*.ts` (4 files, **not** `terminal-work-status-tracker.ts`'s regex), `apps/runtime/src/shell-history/shell-history.ts`

**Interfaces:**
- Consumes: Task 11.
- Produces: default names via `runtimeMessages().hierarchy.newTask`, `.defaultTask`, `sessionCanvas.newCanvas`, `session.untitledClaudeSession`, `providerConfig.builtIn.anthropic` / `.openai` / `.cliDefault`; `shellHistory.restoredDivider`; `sessionCanvas.recoveryFailedTitle` (`'Claude Code 恢复失败'` → `'Claude Code recovery failed'`).

- [ ] **Step 1: Extract.** Default names are read at creation time only (`hierarchy-application-service.ts:2722`, `session-canvas-service.ts:1017`, `claude-session-catalog.ts:323`, `provider-config-store.ts:42-46,234`); existing rows are untouched. The `runtime-server.ts:2219/2233/2334` terminal text `[Fork 未完成，请检查上方原因后重试]` was converted in Task 11; `shell-history.ts` divider becomes `runtimeMessages().shellHistory.restoredDivider` → `'──────── Session restored ────────'` in English.
- [ ] **Step 2: Run** `cd apps/runtime && npx vitest run` — PASS.
- [ ] **Step 3: Add** `apps/runtime/src/i18n/messages.test.ts` asserting that `en` and `zhCN` have identical key sets recursively (guards against a namespace file exporting mismatched shapes through a cast):

```ts
import { describe, expect, it } from 'vitest'
import { en, zhCN } from './messages'
const keys = (value: unknown, prefix = ''): string[] =>
  value && typeof value === 'object'
    ? Object.entries(value).flatMap(([k, v]) => keys(v, `${prefix}${k}.`))
    : [prefix.slice(0, -1)]
describe('runtime catalogs', () => {
  it('have the same keys in both locales', () => { expect(keys(en).sort()).toEqual(keys(zhCN).sort()) })
})
```

- [ ] **Step 4: `pnpm typecheck`; commit** `feat(runtime): localise session canvas, provider config, storage and default names`.

---

### Task 13: Integration, English smoke test, docs

**Files:**
- Create: `tests/e2e/locale-english.spec.ts`
- Create: `apps/desktop/src/renderer/src/i18n/messages/messages.test.ts` (same key-set check as Task 12 Step 3, for the renderer)
- Modify: `README.md` (drop "The product UI is currently in Chinese." from the status line; FAQ "Is the UI in English?" → yes, follows the system language, tray override), `README.zh-CN.md` (same FAQ entry)

**Interfaces:**
- Consumes: everything above; Task 5's `New task` accessible name.

- [ ] **Step 1: Write the smoke test**

```ts
// tests/e2e/locale-english.spec.ts
import { expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('renders the English UI when MATOU_LOCALE=en', async () => {
  const fixture = await launchMatou({ env: { MATOU_LOCALE: 'en' } })
  try {
    await expect(fixture.page.locator('html')).toHaveAttribute('lang', 'en')
    await expect(fixture.page.getByRole('button', { name: 'New task' })).toBeVisible()
  } finally {
    await fixture.close()
  }
})
```

(Use the fixture's actual launch helper name if it differs from `launchMatou`; it is the exported function that calls `startMatou`.)

- [ ] **Step 2: Run everything**

```bash
pnpm typecheck
pnpm test
pnpm check:identifiers
pnpm build
npx playwright test tests/e2e/locale-english.spec.ts tests/e2e/prd-05-hierarchy.spec.ts tests/e2e/prd-02-bottom-hud.spec.ts --reporter=line
```

Expected: all PASS.

- [ ] **Step 3: Manual check on this Mac**

```bash
MATOU_LOCALE=en pnpm dev
```

Expected: English chrome, tray shows "Language" with "Follow system / 中文 / English"; switching to 中文 flips the renderer immediately.

- [ ] **Step 4: Docs and commit**

```bash
git add tests/e2e/locale-english.spec.ts apps/desktop/src/renderer/src/i18n/messages/messages.test.ts README.md README.zh-CN.md
git commit -m "feat: English UI following the system language"
```

Then open a PR from `launch/english-first` (or push to `main` as delegated), and close issue #4 with a note pointing at the tray override.
