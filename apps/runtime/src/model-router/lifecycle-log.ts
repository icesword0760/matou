import { constants } from 'node:fs'
import { open } from 'node:fs/promises'

const MAX_BYTES = 256 * 1024

export async function appendRouterLifecycleLog(path: string, entry: object): Promise<void> {
  const line = JSON.stringify({ timestamp: new Date().toISOString(), ...entry }) + '\n'
  const { O_APPEND, O_CREAT, O_NOFOLLOW, O_NONBLOCK, O_WRONLY } = constants
  const file = await open(path, O_APPEND | O_CREAT | O_NOFOLLOW | O_NONBLOCK | O_WRONLY, 0o600)
  try {
    const stats = await file.stat()
    if (!stats.isFile()) return
    if ((stats.mode & 0o777) !== 0o600) await file.chmod(0o600)
    if (stats.size + Buffer.byteLength(line) > MAX_BYTES) await file.truncate()
    await file.write(line)
  } finally {
    await file.close()
  }
}
