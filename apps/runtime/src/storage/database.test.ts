import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './database'

const opened: RuntimeDatabase[] = []

afterEach(() => {
  for (const database of opened.splice(0)) {
    database.close()
  }
})

async function openDatabase(): Promise<{ database: RuntimeDatabase; path: string }> {
  const directory = await mkdtemp(join(tmpdir(), 'matou-db-'))
  const path = join(directory, 'matou.sqlite')
  const database = RuntimeDatabase.open(path)
  opened.push(database)
  return { database, path }
}

describe('RuntimeDatabase', () => {
  it('configures the durability and isolation pragmas', async () => {
    const { database } = await openDatabase()

    expect(database.pragmas()).toEqual({
      journalMode: 'wal',
      foreignKeys: true,
      synchronous: 2,
      busyTimeout: 5000,
      trustedSchema: false
    })
  })

  it('records a unique runtime generation in one atomic owner file', async () => {
    const { database, path } = await openDatabase()

    expect(database.runtimeGeneration).toMatch(/^[0-9a-f-]{36}$/)
    expect((await stat(`${path}.owner`)).isFile()).toBe(true)
    const owner = JSON.parse(await readFile(`${path}.owner`, 'utf8')) as {
      pid: number
      runtimeGeneration: string
    }
    expect(owner).toEqual({
      pid: process.pid,
      runtimeGeneration: database.runtimeGeneration
    })
  })

  it('rolls back every mutation when a transaction callback throws', async () => {
    const { database } = await openDatabase()
    database.exec('CREATE TABLE values_table (value TEXT NOT NULL)')

    expect(() =>
      database.transaction((tx) => {
        tx.run('INSERT INTO values_table (value) VALUES (?)', 'discard-me')
        throw new Error('boom')
      })
    ).toThrow('boom')

    expect(database.all<{ value: string }>('SELECT value FROM values_table')).toEqual([])
  })

  it('rejects asynchronous transaction callbacks so handles cannot escape', async () => {
    const { database } = await openDatabase()

    expect(() => database.transaction(async () => 'late')).toThrow(
      'database transaction callbacks must be synchronous'
    )
  })

  it('serializes queued writes in submission order', async () => {
    const { database } = await openDatabase()
    database.exec('CREATE TABLE ordered_values (position INTEGER NOT NULL)')
    const completed: number[] = []

    const first = database.enqueueWrite(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20))
      database.transaction((tx) => tx.run('INSERT INTO ordered_values VALUES (?)', 1))
      completed.push(1)
    })
    const second = database.enqueueWrite(async () => {
      database.transaction((tx) => tx.run('INSERT INTO ordered_values VALUES (?)', 2))
      completed.push(2)
    })

    await Promise.all([first, second])
    expect(completed).toEqual([1, 2])
    expect(database.all<{ position: number }>('SELECT position FROM ordered_values')).toEqual([
      { position: 1 },
      { position: 2 }
    ])
  })

  it('prevents a second Runtime owner for the same database', async () => {
    const { path } = await openDatabase()

    expect(() => RuntimeDatabase.open(path)).toThrow('database is already owned by a live Runtime')
  })

  it('rejects an atomic owner record whose pid is still alive', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-live-owner-'))
    const path = join(directory, 'matou.sqlite')
    await writeFile(`${path}.owner`, JSON.stringify({
      pid: process.pid,
      runtimeGeneration: 'live-owner-token'
    }))
    let ownership: ReturnType<typeof RuntimeDatabase.acquireOwnership> | undefined

    try {
      expect(() => {
        ownership = RuntimeDatabase.acquireOwnership(path)
      }).toThrow('database is already owned by a live Runtime')
    } finally {
      ownership?.release()
    }
  })

  it('removes a stale atomic owner and retries the exclusive claim', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-stale-owner-'))
    const path = join(directory, 'matou.sqlite')
    await writeFile(`${path}.owner`, JSON.stringify({
      pid: 2_147_483_647,
      runtimeGeneration: 'stale-owner-token'
    }))

    const database = RuntimeDatabase.open(path)
    opened.push(database)

    expect(JSON.parse(await readFile(`${path}.owner`, 'utf8'))).toEqual({
      pid: process.pid,
      runtimeGeneration: database.runtimeGeneration
    })
  })

  it('releases an owner file only when its generation token still matches', async () => {
    const { database, path } = await openDatabase()
    await rm(`${path}.owner`, { recursive: true, force: true })
    await writeFile(`${path}.owner`, JSON.stringify({
      pid: process.pid,
      runtimeGeneration: 'replacement-owner-token'
    }))

    database.close()
    opened.splice(opened.indexOf(database), 1)

    expect(JSON.parse(await readFile(`${path}.owner`, 'utf8'))).toEqual({
      pid: process.pid,
      runtimeGeneration: 'replacement-owner-token'
    })
  })

  it('allows exactly one owner across processes released from the same barrier', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-owner-race-'))
    const first = spawnOwnerFixture(root, 'first')
    const second = spawnOwnerFixture(root, 'second')

    await waitForFiles(join(root, 'ready-first'), join(root, 'ready-second'))
    await writeFile(join(root, 'acquire-barrier'), 'go')
    await waitForFiles(join(root, 'result-first.json'), join(root, 'result-second.json'))

    const results = await Promise.all(['first', 'second'].map(async (id) =>
      JSON.parse(await readFile(join(root, `result-${id}.json`), 'utf8')) as {
        id: string
        owned: boolean
        runtimeGeneration?: string
      }
    ))
    const owners = results.filter(({ owned }) => owned)
    const ownerIsFile = (await stat(join(root, 'matou.sqlite.owner'))).isFile()
    const ownerRecord = ownerIsFile
      ? JSON.parse(await readFile(join(root, 'matou.sqlite.owner'), 'utf8')) as {
          runtimeGeneration?: string
        }
      : undefined
    await writeFile(join(root, 'release-owner'), 'release')
    await expect(Promise.all([first.completed, second.completed])).resolves.toEqual([0, 0])

    expect(owners).toHaveLength(1)
    expect(ownerIsFile).toBe(true)
    expect(ownerRecord).toMatchObject({ runtimeGeneration: owners[0]!.runtimeGeneration })
  })

  it('releases ownership when closed', async () => {
    const { database, path } = await openDatabase()
    database.close()
    opened.splice(opened.indexOf(database), 1)

    const reopened = RuntimeDatabase.open(path)
    opened.push(reopened)
    expect(reopened.runtimeGeneration).not.toBe(database.runtimeGeneration)
  })
})

function spawnOwnerFixture(root: string, id: string): {
  completed: Promise<number | null>
} {
  const fixturePath = resolve(
    process.cwd(), 'src/storage/database-owner-process.fixture.mjs'
  )
  const child = spawn(process.execPath, [
    '--experimental-strip-types', fixturePath
  ], {
    cwd: process.cwd(),
    env: { ...process.env, MATOU_OWNER_RACE_ROOT: root, MATOU_OWNER_RACE_ID: id },
    stdio: ['ignore', 'pipe', 'pipe']
  })
  let output = ''
  child.stdout.on('data', (chunk) => { output += String(chunk) })
  child.stderr.on('data', (chunk) => { output += String(chunk) })
  return {
    completed: new Promise((resolveExit, reject) => {
      child.once('error', reject)
      child.once('exit', (code) => {
        if (code !== 0) reject(new Error(`owner fixture ${id} failed:\n${output}`))
        else resolveExit(code)
      })
    })
  }
}

async function waitForFiles(...paths: string[]): Promise<void> {
  const deadline = Date.now() + 5_000
  while (true) {
    if ((await Promise.all(paths.map((path) => stat(path).catch(() => undefined))))
      .every((metadata) => metadata?.isFile())) return
    if (Date.now() >= deadline) throw new Error(`files did not appear: ${paths.join(', ')}`)
    await new Promise((resolveWait) => setTimeout(resolveWait, 10))
  }
}
