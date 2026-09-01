import { randomUUID } from 'node:crypto'
import { cp, mkdir, stat, writeFile } from 'node:fs/promises'
import { basename, join } from 'node:path'

export async function exportReadOnlyDatabaseBundle(
  databasePath: string,
  destinationRoot: string,
  now = Date.now()
): Promise<string> {
  const exportPath = join(destinationRoot, `${now}-${randomUUID()}`)
  await mkdir(exportPath, { recursive: true })
  const candidates = [databasePath, `${databasePath}-wal`, `${databasePath}-shm`]
  const exportedFiles: string[] = []
  for (const source of candidates) {
    if (!await isFile(source)) continue
    const name = basename(source)
    await cp(source, join(exportPath, name), { errorOnExist: true })
    exportedFiles.push(name)
  }
  if (exportedFiles.length === 0) throw new Error('没有可导出的数据库文件')
  await writeFile(join(exportPath, 'manifest.json'), JSON.stringify({
    mode: 'read-only', sourceDatabasePath: databasePath, exportedAt: now, exportedFiles
  }, null, 2), { encoding: 'utf8', mode: 0o600 })
  return exportPath
}

async function isFile(path: string): Promise<boolean> {
  try { return (await stat(path)).isFile() } catch { return false }
}
