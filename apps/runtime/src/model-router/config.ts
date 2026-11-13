import { randomBytes } from 'node:crypto'
import { chmod, mkdir, readFile, realpath, rename, writeFile } from 'node:fs/promises'
import { isAbsolute, join, relative, sep } from 'node:path'
import type { ModelAlias } from './claude-launch'
export type RouterModelAliases = Partial<Record<'codex' | 'antigravity', ModelAlias[]>>

export const ROUTER_VERSION = '8.0.69'
export interface RouterKeys { client: string; management: string }

/** Both arguments are canonical paths. An inherited symlink in dataRoot is
 * allowed; a router resource resolving outside that root is not. */
function assertRouterPathWithinDataRoot(dataRoot: string, path: string): void {
  const child = relative(dataRoot, path)
  if (!child || child === '..' || child.startsWith(`..${sep}`) || isAbsolute(child)) throw new Error('Invalid model router directory')
}

export async function prepareRouterDirectory(dataRoot: string): Promise<{ directory: string; keys: RouterKeys }> {
  await mkdir(dataRoot, { recursive: true, mode: 0o700 })
  const canonicalRoot = await realpath(dataRoot)
  const requestedDirectory = join(canonicalRoot, 'model-router')
  await mkdir(requestedDirectory, { recursive: true, mode: 0o700 })
  const directory = await realpath(requestedDirectory)
  assertRouterPathWithinDataRoot(canonicalRoot, directory)
  for (const path of [directory, join(directory, 'credentials'), join(directory, 'logs')]) {
    await mkdir(path, { recursive: true, mode: 0o700 })
    const canonical = await realpath(path)
    assertRouterPathWithinDataRoot(canonicalRoot, canonical)
    await chmod(canonical, 0o700)
  }
  const path = join(directory, 'keys.json')
  let keys: RouterKeys
  try {
    assertRouterPathWithinDataRoot(canonicalRoot, await realpath(path))
    keys = JSON.parse(await readFile(path, 'utf8')) as RouterKeys
    if (![keys.client, keys.management].every(value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value))) throw new Error('Invalid model router keys')
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
    keys = { client: randomBytes(32).toString('hex'), management: randomBytes(32).toString('hex') }
    await writeFile(path, JSON.stringify(keys), { mode: 0o600, flag: 'wx' })
  }
  await chmod(path, 0o600)
  return { directory, keys }
}

export function routerConfig(directory: string, port: number, keys: RouterKeys, proxyUrl?: string, aliases: RouterModelAliases = {}) {
  return {
    'config-version': 8,
    server: { host: '127.0.0.1', port, 'shutdown-timeout': 1, discovery: { enabled: false } },
    management: { 'allow-remote': false, 'secret-key': keys.management, 'disable-control-panel': true, 'disable-auto-update-panel': true },
    access: { 'api-keys': [keys.client] },
    ...(proxyUrl ? { requests: { 'proxy-url': proxyUrl } } : {}),
    oauth: { 'auth-dir': join(directory, 'credentials'), providers: { antigravity: { 'antigravity-credits': false } }, ...(Object.keys(aliases).length ? { 'model-alias': aliases } : {}) },
    client: { claude: { 'show-codex-commentary': true } },
    multimedia: { 'disable-image-generation': 'passthrough' },
    observability: { logs: { debug: false, 'logging-to-file': false, 'request-log': false, 'error-logs-max-files': 10 }, usage: { 'usage-statistics-enabled': false }, pprof: { enable: false } },
    plugins: { enabled: false },
    routing: {
      'force-model-prefix': true,
      retry: { 'request-retry': 0, 'max-retry-credentials': 1, 'max-retry-interval': 0 },
      // In fixed v8.0.69 zero means 60s; -1 disables only transient failures.
      // Keep quota cooling enabled so upstream 429 wait information survives.
      cooldown: { 'disable-cooling': false, 'save-cooldown-status': false, 'transient-error-cooldown-seconds': -1 }
    }
    // Upstream still fetches Grok CLI version metadata at startup / every 3h.
    // This accepted behavior does not update the pinned binary.
  }
}

export async function writeRouterConfig(directory: string, port: number, keys: RouterKeys, proxyUrl?: string, aliases: RouterModelAliases = {}): Promise<void> {
  const path = join(directory, 'config.json')
  const temporary = `${path}.tmp`
  await writeFile(temporary, JSON.stringify(routerConfig(directory, port, keys, proxyUrl, aliases), null, 2), { mode: 0o600 })
  await chmod(temporary, 0o600)
  await rename(temporary, path)
}
