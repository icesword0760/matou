import { createHash, randomUUID } from 'node:crypto'
import { access, chmod, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { constants } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'

export const ANTIGRAVITY_PLUGIN_VERSION = '1.0.0'

const PLUGIN_MANIFEST_JSON = JSON.stringify({
  name: 'matou',
  version: ANTIGRAVITY_PLUGIN_VERSION,
  description: 'Matou lifecycle integration for Antigravity CLI'
}, null, 2) + '\n'

const HOOKS_JSON = JSON.stringify({
  matou: {
    SessionStart: [{ command: './hooks/session-start.sh' }],
    PreInvocation: [{ command: './hooks/matou-hook.sh' }],
    PostInvocation: [{ command: './hooks/matou-hook.sh' }],
    PreToolUse: [{ hooks: [{ command: './hooks/matou-hook.sh' }] }],
    PostToolUse: [{ hooks: [{ command: './hooks/matou-hook.sh' }] }],
    Stop: [{ command: './hooks/matou-hook.sh' }]
  }
}, null, 2) + '\n'

const MATOU_HOOK_SH = `#!/bin/sh
[ -n "\${MATOU_PROVIDER_HOOK_URL:-}" ] || exit 0
[ -n "\${MATOU_PROVIDER_HOOK_TOKEN:-}" ] || exit 0
payload=$(cat)
case "$payload" in
  *'"hookEventName":"PreToolUse"'*|*'"hookEventName": "PreToolUse"'*)
    case "$MATOU_PROVIDER_HOOK_URL" in
      *\\?*) decision_url="\${MATOU_PROVIDER_HOOK_URL}&kind=pre-tool-use" ;;
      *) decision_url="\${MATOU_PROVIDER_HOOK_URL}?kind=pre-tool-use" ;;
    esac
    decision=$(printf '%s' "$payload" | /usr/bin/curl --silent --fail --max-time 1 \\
      --request POST --header 'content-type: application/json' \\
      --data-binary @- "$decision_url" 2>/dev/null) || exit 0
    [ -n "$decision" ] && printf '%s' "$decision"
    exit 0
    ;;
esac
(
  printf '%s' "$payload" | /usr/bin/curl --silent --show-error --max-time 2 \\
    --request POST --header 'content-type: application/json' \\
    --data-binary @- "\${MATOU_PROVIDER_HOOK_URL}" >/dev/null 2>&1
) </dev/null >/dev/null 2>&1 &
exit 0
`

const SESSION_START_SH = `#!/bin/sh
[ -n "\${MATOU_PROVIDER_HOOK_URL:-}" ] || exit 0
[ -n "\${MATOU_PROVIDER_HOOK_TOKEN:-}" ] || exit 0
payload=$(cat)
(
  printf '%s' "$payload" | /usr/bin/curl --silent --show-error --max-time 2 \\
    --request POST --header 'content-type: application/json' \\
    --data-binary @- "\${MATOU_PROVIDER_HOOK_URL}" >/dev/null 2>&1
) </dev/null >/dev/null 2>&1 &
if [ -n "\${MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE:-}" ] && [ -r "$MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE" ]; then
  cat "$MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE"
fi
exit 0
`

const PLUGIN_FILES: ReadonlyArray<{ relativePath: string; content: string; mode: number }> = [
  { relativePath: 'plugin.json', content: PLUGIN_MANIFEST_JSON, mode: 0o644 },
  { relativePath: 'hooks.json', content: HOOKS_JSON, mode: 0o644 },
  { relativePath: 'hooks/matou-hook.sh', content: MATOU_HOOK_SH, mode: 0o755 },
  { relativePath: 'hooks/session-start.sh', content: SESSION_START_SH, mode: 0o755 }
]

const EXPECTED_VERSION_STAMP = `${ANTIGRAVITY_PLUGIN_VERSION}:${createHash('sha256')
  .update(PLUGIN_FILES.map((file) => `${file.relativePath}\0${file.mode}\0${file.content}`).join('\0'))
  .digest('hex')}\n`

export function defaultAntigravityPluginDirectory(
  environment: NodeJS.ProcessEnv = process.env,
  homeDirectory: string = homedir()
): string {
  const configured = environment.MATOU_ANTIGRAVITY_PLUGIN_DIR?.trim()
  return configured || join(homeDirectory, '.gemini', 'config', 'plugins', 'matou')
}

export class AntigravityPluginInstaller {
  readonly pluginDir: string
  #inFlight: Promise<void> | undefined

  constructor(pluginDir: string = defaultAntigravityPluginDirectory()) {
    this.pluginDir = pluginDir
  }

  ensureInstalled(): Promise<void> {
    this.#inFlight ??= this.#install().finally(() => {
      this.#inFlight = undefined
    })
    return this.#inFlight
  }

  async #install(): Promise<void> {
    if (await this.#isUpToDate()) return
    await mkdir(join(this.pluginDir, 'hooks'), { recursive: true, mode: 0o755 })
    for (const file of PLUGIN_FILES) {
      await writeAtomicFile(join(this.pluginDir, file.relativePath), file.content, file.mode)
    }
    await writeAtomicFile(join(this.pluginDir, '.matou-plugin-version'), EXPECTED_VERSION_STAMP, 0o644)
  }

  async #isUpToDate(): Promise<boolean> {
    try {
      const stamp = await readFile(join(this.pluginDir, '.matou-plugin-version'), 'utf8')
      if (stamp !== EXPECTED_VERSION_STAMP) return false
      for (const file of PLUGIN_FILES) {
        const modeFlag = (file.mode & 0o111) !== 0 ? constants.X_OK : constants.R_OK
        await access(join(this.pluginDir, file.relativePath), modeFlag)
      }
      return true
    } catch {
      return false
    }
  }
}

async function writeAtomicFile(targetPath: string, content: string, mode: number): Promise<void> {
  const parentDir = dirname(targetPath)
  await mkdir(parentDir, { recursive: true, mode: 0o755 })
  const tempPath = `${targetPath}.${randomUUID()}.tmp`
  try {
    await writeFile(tempPath, content, { encoding: 'utf8', mode })
    await chmod(tempPath, mode)
    await rename(tempPath, targetPath)
  } catch (error) {
    await rm(tempPath, { force: true }).catch(() => undefined)
    throw error
  }
}
