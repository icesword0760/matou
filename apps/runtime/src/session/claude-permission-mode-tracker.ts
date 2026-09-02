import type { HudPermissionMode } from './session-hud-registry'

const PERMISSION_FOOTER = /(bypass permissions|auto mode|plan mode|accept edits(?: mode)?|default mode)\s+on\s*\(shift\+tab to cycle\)/gi

export class ClaudePermissionModeTracker {
  #buffer = ''
  #current: HudPermissionMode | undefined

  ingest(chunk: string): HudPermissionMode | undefined {
    this.#buffer = visibleTerminalText(this.#buffer + chunk).slice(-8_192)
    let latest: HudPermissionMode | undefined
    PERMISSION_FOOTER.lastIndex = 0
    for (const match of this.#buffer.matchAll(PERMISSION_FOOTER)) latest = permissionMode(match[1] ?? '')
    if (!latest || latest === this.#current) return undefined
    this.#current = latest
    return latest
  }
}

function permissionMode(value: string): HudPermissionMode | undefined {
  const normalized = value.toLowerCase()
  if (normalized === 'bypass permissions') return 'bypassPermissions'
  if (normalized === 'auto mode') return 'auto'
  if (normalized === 'plan mode') return 'plan'
  if (normalized.startsWith('accept edits')) return 'acceptEdits'
  if (normalized === 'default mode') return 'default'
  return undefined
}

function visibleTerminalText(raw: string): string {
  return raw
    .replace(/\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g, '')
    .replace(/\u001b\[[0-?]*[ -\/]*[@-~]/g, '')
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, '')
}
