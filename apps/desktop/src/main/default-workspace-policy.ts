import { basename } from 'node:path'

export function resolveDefaultWorkspacePath(
  override: string | undefined,
  homeDirectory: string
): { rootDirectory: string; name: string } {
  const rootDirectory = override ?? homeDirectory
  return { rootDirectory, name: basename(rootDirectory) }
}
