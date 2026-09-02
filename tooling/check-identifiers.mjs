import { readFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'

const forbidden = [
  ['koo', 'ky'].join(''),
  ['wa', 'rp'].join('')
]
const base = process.env.MATOU_IDENTIFIER_BASE ?? 'ecc3d28'
const violations = []

for (const path of untrackedPaths()) scanPath(path, readFile(path).split('\n'))

let currentPath = ''
const addedByPath = new Map()
for (const line of git(['diff', '--no-color', '--unified=0', base, '--'])) {
  if (line.startsWith('+++ b/')) {
    currentPath = line.slice(6)
    continue
  }
  if (!currentPath || !line.startsWith('+') || line.startsWith('+++')) continue
  const additions = addedByPath.get(currentPath) ?? []
  additions.push(line.slice(1))
  addedByPath.set(currentPath, additions)
}
for (const [path, additions] of addedByPath) scanPath(path, additions)

if (violations.length > 0) {
  console.error('Identifier hygiene check failed:')
  for (const violation of violations) console.error(`- ${violation}`)
  process.exit(1)
}
console.log(`Identifier hygiene check passed for changes since ${base.slice(0, 12)}.`)

function scanPath(path, lines) {
  scan(path, `path ${path}`)
  lines.forEach((line, index) => scan(line, `${path}:added-${index + 1}`))
}

function scan(value, location) {
  const normalized = value.toLowerCase()
  for (const identifier of forbidden) {
    if (normalized.includes(identifier)) violations.push(`${location} contains ${identifier}`)
  }
}

function readFile(path) {
  try {
    return readFileSync(path, 'utf8')
  } catch {
    return ''
  }
}

function untrackedPaths() {
  return git(['ls-files', '--others', '--exclude-standard']).filter(Boolean)
}

function git(args) {
  return runGit(args).split('\n')
}

function runGit(args) {
  const result = spawnSync('git', args, { encoding: 'utf8' })
  if (result.status !== 0) {
    throw new Error(result.stderr || `git ${args.join(' ')} failed`)
  }
  return result.stdout
}
