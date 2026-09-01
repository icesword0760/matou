import { execFileSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const retiredIdentifierPatterns = [
  new RegExp(['c', 'm', 'u', 'x'].join(''), 'i'),
  new RegExp(['k', 'o', 'o', 'k', 'y'].join(''), 'i'),
  new RegExp(['w', 'a', 'r', 'p'].join(''), 'i')
]
const generatedContentPaths = new Set(['pnpm-lock.yaml', 'package-lock.json', 'yarn.lock'])

function containsRetiredIdentifier(value) {
  return retiredIdentifierPatterns.some((pattern) => pattern.test(value))
}

export async function scanProjectPaths(root, paths) {
  const violations = []
  for (const path of paths) {
    if (containsRetiredIdentifier(path)) {
      violations.push({ location: path, kind: 'path' })
      continue
    }
    if (generatedContentPaths.has(path)) continue

    const content = await readFile(resolve(root, path), 'utf8')
    if (containsRetiredIdentifier(content)) {
      violations.push({ location: path, kind: 'content' })
    }
  }
  return violations
}

async function main() {
  const root = process.cwd()
  const paths = execFileSync(
    'git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'],
    { cwd: root }
  ).toString('utf8').split('\0').filter(Boolean)
  const violations = await scanProjectPaths(root, paths)
  if (violations.length === 0) return

  process.stderr.write('Retired external product identifiers found:\n')
  for (const violation of violations) {
    process.stderr.write(`- ${violation.location} (${violation.kind})\n`)
  }
  process.exitCode = 1
}

const isDirectRun = process.argv[1]
  && resolve(process.argv[1]) === fileURLToPath(import.meta.url)
if (isDirectRun) await main()
