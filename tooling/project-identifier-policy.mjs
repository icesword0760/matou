import { execFileSync } from 'node:child_process'
import { readFile, stat } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const retiredIdentifiers = [
  ['c', 'm', 'u', 'x'].join(''),
  ['k', 'o', 'o', 'k', 'y'].join('')
]

export function isGeneratedArtifactPath(path) {
  return path.startsWith('.playwright-cli/')
    || path.startsWith('output/')
    || /^apps\/desktop\/release(?:-|\/)/.test(path)
}

export async function scanProjectPaths(root, paths) {
  const violations = []
  for (const path of paths) {
    const normalizedPath = path.toLowerCase()
    if (retiredIdentifiers.some((identifier) => normalizedPath.includes(identifier))) {
      violations.push({ location: path, kind: 'path' })
      continue
    }

    if (!(await stat(resolve(root, path))).isFile()) continue
    const content = (await readFile(resolve(root, path))).toString('latin1').toLowerCase()
    if (retiredIdentifiers.some((identifier) => content.includes(identifier))) {
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
  const violations = await scanProjectPaths(
    root,
    paths.filter((path) => !isGeneratedArtifactPath(path))
  )
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
