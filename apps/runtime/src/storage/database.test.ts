import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

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

  it('records a unique runtime generation and an owner record', async () => {
    const { database, path } = await openDatabase()

    expect(database.runtimeGeneration).toMatch(/^[0-9a-f-]{36}$/)
    const owner = JSON.parse(await readFile(`${path}.owner/owner.json`, 'utf8')) as {
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

  it('releases ownership when closed', async () => {
    const { database, path } = await openDatabase()
    database.close()
    opened.splice(opened.indexOf(database), 1)

    const reopened = RuntimeDatabase.open(path)
    opened.push(reopened)
    expect(reopened.runtimeGeneration).not.toBe(database.runtimeGeneration)
  })
})
