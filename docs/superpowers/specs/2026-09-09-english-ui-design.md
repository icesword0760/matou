# English UI (i18n) Design

Date: 2026-09-09. Status: approved for implementation (owner delegated execution to Claude).

## Goal

Ship an English user interface that follows the system language, with a manual override, without regressing the Chinese UI or the ~2,000 Chinese assertions in unit and end-to-end tests. Blocker for the English launch (Show HN / Reddit) scheduled for 2026-09-23. Tracking issue: [#4](https://github.com/icesword0760/matou/issues/4).

## Scope

In scope (survey of 2026-09-09):

| Area | Files | Chinese string sites |
|---|---|---|
| Renderer (`apps/desktop/src/renderer/src`) | 45 | ~782 |
| Main process (`apps/desktop/src/main`) | 6 | 16 |
| Runtime (`apps/runtime/src`, excluding `cli/mt-cli.ts`) | 30 | ~161 |

Out of scope:

- `apps/runtime/src/cli/mt-cli.ts` (115 strings). `mt` is read by the hosted AI agent, not the person; its help text is already English. Left as is.
- `productName` (`码头`), the `进入码头.workflow` Quick Action file name, `APP_STORAGE_DIRECTORY_NAME`. Renaming the bundle breaks the installed Services entry and update paths.
- Chinese used as *data*: the waiting-for-input regex in `terminal-work-status-tracker.ts`, the signature-error regex in `app-update-manager.ts`, the display-name regex in `e2e-window-placement.ts`. English patterns may be added, Chinese ones are not removed.
- Retroactive translation of persisted records (task titles, canvas names, provider names, text already written into terminal scrollback). New records are created in the active locale; old ones keep their text.
- A settings page. The manual override lives in the tray menu for this iteration.

## Locale model

```ts
// packages/contracts/src/locale.ts
export const LOCALES = ['zh-CN', 'en'] as const
export type Locale = (typeof LOCALES)[number]
export type LocalePreference = Locale | 'system'
export const DEFAULT_LOCALE: Locale = 'zh-CN'
```

Resolution order, highest priority first:

1. `MATOU_LOCALE` environment variable, if it is a valid `Locale`. Used by tests and by the runtime process.
2. Stored preference in `<userData>/locale.json` (`{"preference":"en"}`), if it is a `Locale`.
3. System locale from `app.getLocale()`: anything starting with `zh` → `zh-CN`, everything else → `en`.
4. `DEFAULT_LOCALE` (`zh-CN`) when nothing is known. This is what unit tests see, because they never have a desktop API.

## Process responsibilities

- **Main** owns resolution (`LocaleStore`), persists the preference, rebuilds the tray menu on change, sets the DAG window title, and spawns the runtime with `MATOU_LOCALE=<resolved locale>`. Exposes `getLocale`, `getLocalePreference`, `setLocalePreference`, `onLocaleChanged` over the existing `DESKTOP_CHANNELS` / `MatouDesktopApi` bridge.
- **Renderer** reads the locale from the bridge once, subscribes to changes, sets `<html lang>`, and renders from a typed message catalog. It never reads `navigator.language`.
- **Runtime** reads `MATOU_LOCALE` once at start. Its user-facing strings (protocol error payloads, notification titles/bodies, default entity names, text written into terminals) come from a runtime catalog. A locale change takes effect in the runtime after the app restarts; the tray menu says so.

## Catalog design

Typed TypeScript objects, one namespace per UI area, Chinese as the source shape and English typed against it:

```ts
// apps/desktop/src/renderer/src/i18n/catalog.ts
export type CatalogShape<T> = {
  [K in keyof T]: T[K] extends (...args: infer A) => string
    ? (...args: A) => string
    : T[K] extends string ? string : CatalogShape<T[K]>
}
```

- Static text is a string; text with variables is a function `(…) => string`. English plurals are handled inside the function (`n === 1 ? '1 session' : \`${n} sessions\``). No runtime key lookup, no missing-key failure mode: a missing English key is a type error.
- Namespaces (renderer): `common`, `app`, `hierarchyShell`, `hierarchyTerminal`, `hud`, `sessionCanvas`, `updates`, `recovery`, `dag`, `notifications`, `terminal`.
- Namespaces (runtime): `control`, `server`, `sessionCanvas`, `providerConfig`, `hierarchy`, `storage`, `git`, `session`, `shellHistory`.
- Components call `const m = useMessages()`. Non-React modules (`terminal-close-flow.ts`, `hierarchy-commands.ts`, notification builders) call `messages()` from `i18n/current.ts`, which the provider keeps in sync.
- `aria-label` / `title` that duplicate the visible label reuse the same key.
- `Intl.DateTimeFormat` and `toLocaleString` calls use the active locale instead of the hardcoded `'zh-CN'`.
- `terminal-loading-state.ts` returns `labelKey: 'loading' | 'recovering'`; the component resolves the text.

## English copy rules

- Brand is **Matou** in English strings (tray "Show Matou", title "Matou · Session DAG"); `码头` stays in Chinese strings.
- Sentence case for labels and buttons ("New task", "Remove from Matou"). No trailing periods on labels; full sentences in dialogs and notifications.
- Vocabulary: workspace, task, canvas, session card / session, fork, child session, worktree, board, notification center, HUD, recovery. "Claude Code" is never translated.
- Straight quotes in English; keep CJK quotes in Chinese.

## Tests

- Unit tests keep passing unchanged: the renderer defaults to `zh-CN` without a desktop API, the runtime defaults to `zh-CN` without `MATOU_LOCALE`, and `vitest` configs pin `MATOU_LOCALE=zh-CN` so a developer shell cannot flip them.
- End-to-end fixtures launch with `MATOU_LOCALE=zh-CN` unless a spec overrides it. One new spec launches with `en` and checks `<html lang="en">` and an English accessible name.
- Each namespace task adds at least one English rendering test.
- `pnpm typecheck`, `pnpm test`, `pnpm check:identifiers` and `pnpm build` must pass before merge.

## Follow-ups (not in this iteration)

- Settings UI for language inside the app.
- English bundle name / Quick Action name.
- Re-capture README screenshots and GIF in English (marketing task, after this lands).
