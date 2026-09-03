import { describe, expect, it } from 'vitest'

import { assertStartupIntegrity } from './database-integrity'

describe('startup database integrity', () => {
  it('uses the fast SQLite startup check', () => {
    const statements: string[] = []
    const database = {
      all(sql: string) {
        statements.push(sql)
        return [{ quick_check: 'ok' }]
      }
    }

    assertStartupIntegrity(database)

    expect(statements).toEqual(['PRAGMA quick_check'])
  })

  it('still rejects physical corruption', () => {
    const database = { all: () => [{ quick_check: 'database disk image is malformed' }] }
    expect(() => assertStartupIntegrity(database)).toThrow('database corrupt')
  })
})
