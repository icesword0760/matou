import { spawn } from 'node:child_process'
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

const { DatabaseSync } = process.getBuiltinModule('node:sqlite')

if (process.argv[2] === '--writer') {
  const root = process.argv[3]
  const databasePath = join(root, 'matou.sqlite')
  const database = new DatabaseSync(databasePath)
  database.exec(`
    PRAGMA journal_mode = WAL;
    PRAGMA wal_autocheckpoint = 0;
    CREATE TABLE wal_values (value TEXT NOT NULL);
    INSERT INTO wal_values VALUES ('base');
    PRAGMA wal_checkpoint(TRUNCATE);
    INSERT INTO wal_values VALUES ('committed-in-wal');
  `)
  writeFileSync(join(root, 'writer-ready'), 'ready')
  setInterval(() => {}, 1_000)
} else {
  const root = mkdtempSync(join(tmpdir(), 'matou-electron-wal-fixture-'))
  const child = spawn(process.execPath, [process.argv[1], '--writer', root], {
    env: process.env,
    stdio: ['ignore', 'ignore', 'inherit']
  })
  while (!existsSync(join(root, 'writer-ready'))) {
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
  const exited = new Promise((resolve, reject) => {
    child.once('exit', resolve)
    child.once('error', reject)
  })
  child.kill('SIGKILL')
  await exited

  const databasePath = join(root, 'matou.sqlite')
  const walPath = `${databasePath}-wal`
  const shmPath = `${databasePath}-shm`
  const existingShm = readRows(databasePath)

  const missingRoot = `${root}-missing-shm`
  await import('node:fs/promises').then(({ mkdir }) => mkdir(missingRoot))
  for (const suffix of ['', '-wal']) {
    copyFileSync(`${databasePath}${suffix}`, join(missingRoot, `matou.sqlite${suffix}`))
  }
  const missingPath = join(missingRoot, 'matou.sqlite')
  chmodSync(missingPath, 0o444)
  chmodSync(`${missingPath}-wal`, 0o444)
  chmodSync(missingRoot, 0o555)
  const missingShm = capture(() => readRows(missingPath))
  const immutableUrl = pathToFileURL(missingPath)
  immutableUrl.searchParams.set('immutable', '1')
  const immutableRows = readRows(immutableUrl)

  process.stdout.write(`${JSON.stringify({
    runtime: process.versions,
    root,
    bundle: {
      mainBytes: statSync(databasePath).size,
      walBytes: statSync(walPath).size,
      shmBytes: statSync(shmPath).size,
      shmHeaderHex: readFileSync(shmPath).subarray(0, 96).toString('hex')
    },
    existingShm,
    missingShm,
    immutableRows
  }, null, 2)}\n`)

  chmodSync(missingRoot, 0o755)
  chmodSync(missingPath, 0o644)
  chmodSync(`${missingPath}-wal`, 0o644)
  rmSync(join(root, 'writer-ready'), { force: true })
}

function readRows(path) {
  const database = new DatabaseSync(path, { readOnly: true })
  try {
    return database.prepare('SELECT value FROM wal_values ORDER BY rowid').all()
  } finally {
    database.close()
  }
}

function capture(operation) {
  try {
    return { ok: true, value: operation() }
  } catch (error) {
    return {
      ok: false,
      error: {
        name: error?.name,
        code: error?.code,
        errcode: error?.errcode,
        message: error instanceof Error ? error.message : String(error)
      }
    }
  }
}
