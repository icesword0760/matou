import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './database'
import { readDatabaseOwner } from './database-owner'

const opened: RuntimeDatabase[] = []
// Full Runtime runs include PTY timing suites in parallel; stress verification sets this to 8.
const OWNER_RACE_CONTENDERS = Number(process.env.MATOU_OWNER_RACE_CONTENDERS ?? 2)
const OWNER_RACE_ITERATIONS = Number(process.env.MATOU_OWNER_RACE_ITERATIONS ?? 2)

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
  it('reads and resets the exact statement count for a scale measurement window', async () => {
    const { database } = await openDatabase()
    database.readStatementCount(true)

    database.exec('CREATE TABLE scale_values (value TEXT NOT NULL)')
    database.run('INSERT INTO scale_values VALUES (?)', 'one')
    database.get<{ value: string }>('SELECT value FROM scale_values LIMIT 1')
    database.all<{ value: string }>('SELECT value FROM scale_values')

    expect(database.readStatementCount()).toBe(4)
    expect(database.readStatementProfile()).toEqual([
      { statement: 'CREATE TABLE scale_values (value TEXT NOT NULL)', count: 1 },
      { statement: 'INSERT INTO scale_values VALUES (?)', count: 1 },
      { statement: 'SELECT value FROM scale_values', count: 1 },
      { statement: 'SELECT value FROM scale_values LIMIT 1', count: 1 }
    ])
    expect(database.readStatementCount(true)).toBe(4)
    expect(database.readStatementCount()).toBe(0)
    expect(database.readStatementProfile()).toEqual([])
  })

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

  it('keeps a live legacy owner directory fail-closed', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-live-legacy-owner-'))
    const path = join(directory, 'matou.sqlite')
    await mkdir(`${path}.owner`)
    await writeFile(`${path}.owner/owner.json`, JSON.stringify({
      pid: process.pid,
      runtimeGeneration: 'live-legacy-owner-token'
    }))

    expect(() => RuntimeDatabase.acquireOwnership(path))
      .toThrow('database is already owned by a live Runtime')
    expect((await stat(`${path}.owner`)).isDirectory()).toBe(true)
  })

  it('safely replaces a dead legacy owner directory with the atomic owner file', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-dead-legacy-owner-'))
    const path = join(directory, 'matou.sqlite')
    await mkdir(`${path}.owner`)
    await writeFile(`${path}.owner/owner.json`, JSON.stringify({
      pid: 2_147_483_647,
      runtimeGeneration: 'dead-legacy-owner-token'
    }))

    const database = RuntimeDatabase.open(path)
    opened.push(database)

    expect((await stat(`${path}.owner`)).isFile()).toBe(true)
    expect(JSON.parse(await readFile(`${path}.owner`, 'utf8'))).toEqual({
      pid: process.pid,
      runtimeGeneration: database.runtimeGeneration
    })
  })

  it.each([0, -1, -42])('rejects non-positive owner pid %s as malformed', async (pid) => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-invalid-owner-pid-'))
    const ownerPath = join(directory, 'matou.sqlite.owner')
    await writeFile(ownerPath, JSON.stringify({ pid, runtimeGeneration: 'invalid-pid-token' }))

    expect(readDatabaseOwner(ownerPath)).toBeUndefined()
    expect(() => RuntimeDatabase.acquireOwnership(join(directory, 'matou.sqlite')))
      .toThrow('database ownership state requires recovery')
  })

  it.each(['prepared', 'published'] as const)(
    'survives a publisher killed after the owner record is %s',
    async (stage) => {
      const root = await mkdtemp(join(tmpdir(), `matou-owner-publisher-${stage}-`))
      const ownerPath = join(root, 'matou.sqlite.owner')
      const publisher = spawnOwnerPublisherFixture(root, stage)
      await waitForFiles(join(root, `publisher-${stage}`))

      publisher.kill()
      await publisher.completed

      if (stage === 'prepared') {
        await expect(readFile(ownerPath)).rejects.toMatchObject({ code: 'ENOENT' })
      } else {
        expect(JSON.parse(await readFile(ownerPath, 'utf8'))).toMatchObject({
          pid: publisher.pid,
          runtimeGeneration: 'publisher-generation'
        })
      }
      const ownership = RuntimeDatabase.acquireOwnership(join(root, 'matou.sqlite'))
      try {
        expect(JSON.parse(await readFile(ownerPath, 'utf8'))).toMatchObject({
          pid: process.pid,
          runtimeGeneration: ownership.runtimeGeneration
        })
      } finally {
        ownership.release()
      }
    },
    20_000
  )

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

  it.each([
    'empty', 'stale-file', 'dead-legacy-directory', 'abandoned-takeover'
  ] as const)(
    'allows exactly one owner across processes racing over %s',
    async (scenario) => {
      for (let iteration = 0; iteration < OWNER_RACE_ITERATIONS; iteration += 1) {
        const result = await runOwnerRaceScenario(scenario, iteration)
        expect(result.owners, `${scenario} iteration ${iteration}`).toHaveLength(1)
        expect(result.ownerIsFile, `${scenario} iteration ${iteration}`).toBe(true)
        expect(result.ownerRecord, `${scenario} iteration ${iteration}`).toMatchObject({
          runtimeGeneration: result.owners[0]!.runtimeGeneration
        })
      }
    },
    60_000
  )

  it('continues stale takeover after the lock-holding process crashes', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-owner-crashed-takeover-'))
    const ownerPath = join(root, 'matou.sqlite.owner')
    await writeFile(ownerPath, JSON.stringify({
      pid: 2_147_483_647,
      runtimeGeneration: 'stale-before-takeover-crash'
    }))
    const holder = spawnTakeoverLockHolder(root)
    await waitForFiles(join(root, 'takeover-lock-ready'))
    const ids = Array.from({ length: OWNER_RACE_CONTENDERS }, (_, index) => String(index))
    const contenders = ids.map((id) => spawnOwnerFixture(root, id))
    await waitForFiles(...ids.map((id) => join(root, `ready-${id}`)))
    await writeFile(join(root, 'acquire-barrier'), 'go')
    await new Promise((resolveWait) => setTimeout(resolveWait, 250))
    const resultsBeforeCrash = await Promise.all(ids.map((id) =>
      stat(join(root, `result-${id}.json`)).then(() => true, () => false)
    ))

    holder.kill()
    await holder.completed
    await waitForFiles(...ids.map((id) => join(root, `result-${id}.json`)))
    const results = await Promise.all(ids.map(async (id) =>
      JSON.parse(await readFile(join(root, `result-${id}.json`), 'utf8')) as {
        id: string
        owned: boolean
        runtimeGeneration?: string
      }
    ))
    const canonical = JSON.parse(await readFile(ownerPath, 'utf8')) as {
      runtimeGeneration?: string
    }
    await writeFile(join(root, 'release-owner'), 'release')
    await Promise.all(contenders.map(({ completed }) => completed))

    expect(resultsBeforeCrash).toEqual(Array(OWNER_RACE_CONTENDERS).fill(false))
    const owners = results.filter(({ owned }) => owned)
    expect(owners).toHaveLength(1)
    expect(canonical.runtimeGeneration).toBe(owners[0]!.runtimeGeneration)
  }, 20_000)

  it('releases ownership when closed', async () => {
    const { database, path } = await openDatabase()
    database.close()
    opened.splice(opened.indexOf(database), 1)

    const reopened = RuntimeDatabase.open(path)
    opened.push(reopened)
    expect(reopened.runtimeGeneration).not.toBe(database.runtimeGeneration)
  })
})

async function runOwnerRaceScenario(
  scenario: 'empty' | 'stale-file' | 'dead-legacy-directory' | 'abandoned-takeover',
  iteration: number
): Promise<{
  owners: Array<{ id: string; owned: boolean; runtimeGeneration?: string }>
  ownerIsFile: boolean
  ownerRecord: { runtimeGeneration?: string } | undefined
}> {
  const root = await mkdtemp(join(tmpdir(), `matou-owner-${scenario}-${iteration}-`))
  const ownerPath = join(root, 'matou.sqlite.owner')
  if (scenario === 'stale-file' || scenario === 'abandoned-takeover') {
    await writeFile(ownerPath, JSON.stringify({
      pid: 2_147_483_647,
      runtimeGeneration: `stale-file-${iteration}`
    }))
    if (scenario === 'abandoned-takeover') {
      await writeFile(`${ownerPath}.takeover`, JSON.stringify({
        pid: 2_147_483_647,
        runtimeGeneration: `abandoned-takeover-${iteration}`
      }))
    }
  } else if (scenario === 'dead-legacy-directory') {
    await mkdir(ownerPath)
    await writeFile(join(ownerPath, 'owner.json'), JSON.stringify({
      pid: 2_147_483_647,
      runtimeGeneration: `dead-legacy-${iteration}`
    }))
  }
  const ids = Array.from({ length: OWNER_RACE_CONTENDERS }, (_, index) => String(index))
  const contenders = ids.map((id) => spawnOwnerFixture(root, id))
  await waitForFiles(...ids.map((id) => join(root, `ready-${id}`)))
  await writeFile(join(root, 'acquire-barrier'), 'go')
  await waitForFiles(...ids.map((id) => join(root, `result-${id}.json`)))
  const results = await Promise.all(ids.map(async (id) =>
    JSON.parse(await readFile(join(root, `result-${id}.json`), 'utf8')) as {
      id: string
      owned: boolean
      runtimeGeneration?: string
    }
  ))
  const ownerIsFile = (await stat(ownerPath)).isFile()
  const ownerRecord = ownerIsFile
    ? JSON.parse(await readFile(ownerPath, 'utf8')) as { runtimeGeneration?: string }
    : undefined
  await writeFile(join(root, 'release-owner'), 'release')
  await Promise.all(contenders.map(({ completed }) => completed))
  return { owners: results.filter(({ owned }) => owned), ownerIsFile, ownerRecord }
}

function spawnOwnerFixture(root: string, id: string): {
  completed: Promise<number | null>
} {
  const fixturePath = resolve(
    process.cwd(), 'src/storage/database-owner-process.fixture.mjs'
  )
  const child = spawn(process.execPath, [
    '--experimental-sqlite', '--experimental-strip-types', fixturePath
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

function spawnTakeoverLockHolder(root: string): {
  kill(): void
  completed: Promise<void>
} {
  const fixturePath = resolve(
    process.cwd(), 'src/storage/database-owner-lock-holder.fixture.mjs'
  )
  const child = spawn(process.execPath, [
    '--experimental-sqlite', fixturePath
  ], {
    cwd: process.cwd(),
    env: { ...process.env, MATOU_OWNER_RACE_ROOT: root },
    stdio: ['ignore', 'pipe', 'pipe']
  })
  let output = ''
  child.stdout.on('data', (chunk) => { output += String(chunk) })
  child.stderr.on('data', (chunk) => { output += String(chunk) })
  return {
    kill: () => { child.kill('SIGKILL') },
    completed: new Promise((resolveExit, reject) => {
      child.once('error', reject)
      child.once('exit', (code, signal) => {
        if (signal === 'SIGKILL') resolveExit()
        else reject(new Error(`takeover lock holder exited ${code ?? signal}:\n${output}`))
      })
    })
  }
}

function spawnOwnerPublisherFixture(root: string, stage: 'prepared' | 'published'): {
  pid: number
  kill(): void
  completed: Promise<void>
} {
  const fixturePath = resolve(
    process.cwd(), 'src/storage/database-owner-publisher.fixture.mjs'
  )
  const child = spawn(process.execPath, [
    '--experimental-sqlite', '--experimental-strip-types', fixturePath
  ], {
    cwd: process.cwd(),
    env: { ...process.env, MATOU_OWNER_PUBLISH_ROOT: root, MATOU_OWNER_PUBLISH_STAGE: stage },
    stdio: ['ignore', 'pipe', 'pipe']
  })
  let output = ''
  child.stdout.on('data', (chunk) => { output += String(chunk) })
  child.stderr.on('data', (chunk) => { output += String(chunk) })
  if (child.pid === undefined) throw new Error('owner publisher did not start')
  return {
    pid: child.pid,
    kill: () => { child.kill('SIGKILL') },
    completed: new Promise((resolveExit, reject) => {
      child.once('error', reject)
      child.once('exit', (code, signal) => {
        if (signal === 'SIGKILL') resolveExit()
        else reject(new Error(`owner publisher exited ${code ?? signal}:\n${output}`))
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
