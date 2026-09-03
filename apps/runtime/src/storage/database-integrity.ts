export interface IntegrityCheckDatabase {
  all(sql: string): object[]
}

export function assertStartupIntegrity(database: IntegrityCheckDatabase): void {
  const rows = database.all('PRAGMA quick_check')
  const result = rows.map((row) => String(Object.values(row)[0] ?? ''))
  if (result.length !== 1 || result[0]?.toLowerCase() !== 'ok') {
    throw new Error(`database corrupt: quick_check failed: ${result.slice(0, 3).join('; ')}`)
  }
}
