import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

const prohibited = [
  new RegExp(`\\b${['koo', 'ky'].join('')}\\b`, 'i'),
  new RegExp(`\\b${['wa', 'rp'].join('')}\\b`, 'i')
]

const changed = new Set([
  ...lines(git(['diff', '--name-only', '--diff-filter=ACMR', 'HEAD'])),
  ...lines(git(['ls-files', '--others', '--exclude-standard']))
])
const violations = []

if (changed.size === 0) {
  for (const path of lines(git(['diff-tree', '--no-commit-id', '--name-only', '-r', 'HEAD']))) {
    if (matches(path)) violations.push(`${path}: prohibited identifier in path`)
  }
  scanPatch(git(['show', '--format=', '--unified=0', 'HEAD']), violations)
} else {
  for (const path of changed) {
    if (matches(path)) violations.push(`${path}: prohibited identifier in path`)
    if (isUntracked(path)) {
      scanText(path, readFileSync(path, 'utf8'), violations)
    } else {
      scanPatch(git(['diff', '--unified=0', 'HEAD', '--', path]), violations)
    }
  }
}

if (violations.length > 0) {
  console.error(`Identifier gate failed:\n${violations.map((value) => `- ${value}`).join('\n')}`)
  process.exitCode = 1
} else {
  console.log(`Identifier gate passed (${changed.size || 1} change set).`)
}

function git(args) {
  return execFileSync('git', args, { encoding: 'utf8' })
}

function lines(value) {
  return value.split('\n').map((line) => line.trim()).filter(Boolean)
}

function isUntracked(path) {
  return lines(git(['ls-files', '--others', '--exclude-standard', '--', path])).includes(path)
}

function scanPatch(patch, violations) {
  let path = 'unknown'
  for (const line of patch.split('\n')) {
    if (line.startsWith('+++ b/')) {
      path = line.slice(6)
      continue
    }
    if (!line.startsWith('+') || line.startsWith('+++')) continue
    if (matches(line.slice(1))) violations.push(`${path}: prohibited identifier in added content`)
  }
}

function scanText(path, text, violations) {
  if (matches(text)) violations.push(`${path}: prohibited identifier in new content`)
}

function matches(value) {
  return prohibited.some((pattern) => pattern.test(value))
}
