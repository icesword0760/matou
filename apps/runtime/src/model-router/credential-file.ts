import { readFile } from 'node:fs/promises'
import { isAbsolute, join, relative, resolve } from 'node:path'

/** Read one router credential JSON, but only from the credential directory.
 * Paths outside it are treated as unreadable so identity pinning falls back to
 * writing rather than following a router-supplied path elsewhere. */
export async function readCredentialJson(
  dataRoot: string,
  file: { name: string; path?: unknown }
): Promise<Record<string, unknown> | undefined> {
  const directory = resolve(dataRoot, 'model-router', 'credentials')
  const candidate = typeof file.path === 'string' && file.path.length > 0 ? file.path : join(directory, file.name)
  const path = resolve(candidate)
  const inside = relative(directory, path)
  if (!inside || inside.startsWith('..') || isAbsolute(inside) || !inside.endsWith('.json')) return undefined
  try { return JSON.parse(await readFile(path, 'utf8')) as Record<string, unknown> } catch { return undefined }
}
