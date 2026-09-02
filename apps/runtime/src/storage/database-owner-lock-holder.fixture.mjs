import { writeFileSync } from 'node:fs'
import { join } from 'node:path'

const root = process.env.MATOU_OWNER_RACE_ROOT
if (!root) throw new Error('owner race root is missing')

const { DatabaseSync } = process.getBuiltinModule('node:sqlite')
const lock = new DatabaseSync(join(root, 'matou.sqlite.owner.takeover.sqlite'))
lock.exec('PRAGMA busy_timeout = 5000; BEGIN EXCLUSIVE;')
writeFileSync(join(root, 'takeover-lock-ready'), 'ready')
setInterval(() => {}, 1_000)
