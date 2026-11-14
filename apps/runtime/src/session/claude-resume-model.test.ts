import { randomUUID } from 'node:crypto'
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { resolve, join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { claudeResumeUsesSubscriptionModel } from './claude-resume-model'
import { encodeClaudeProjectPath } from './claude-session-catalog'

const roots: string[] = []
afterEach(async () => { await Promise.all(roots.splice(0).map(root => rm(root, { recursive: true, force: true }))) })
async function fixture(rows: unknown[]) {
  const base = resolve(import.meta.dirname, '../../../../output/model-switch-rework')
  await mkdir(base, { recursive: true })
  const root = await mkdtemp(join(base, 'resume-reader-')); roots.push(root)
  const cwd = join(root, 'project'), config = join(root, 'config'), sessionId = randomUUID()
  const bucket = join(config, 'projects', encodeClaudeProjectPath(cwd))
  await mkdir(cwd); await mkdir(bucket, { recursive: true })
  const file = join(bucket, `${sessionId}.jsonl`), original = rows.map(row => JSON.stringify(row)).join('\n') + '\n'
  await writeFile(file, original)
  return { root, cwd, config, sessionId, file, original, read: (directory = cwd) => claudeResumeUsesSubscriptionModel(directory, sessionId, { CLAUDE_CONFIG_DIR: config }) }
}
const assistant = (model: string) => ({ type: 'assistant', message: { model, content: [] } })
const identity = (modelId: string) => ({ type: 'attachment', attachment: { type: 'model', identity: { modelId } } })
describe('subscription model left in native resume history', () => {
  it('detects the latest subscription assistant without changing the history', async () => {
    const f = await fixture([assistant('claude-sonnet-5'), assistant('matou-chatgpt/matou-opus')])
    expect(await f.read()).toBe(true)
    expect(await readFile(f.file, 'utf8')).toBe(f.original)
  })
  it('uses the saved client identity when the upstream response names GPT', async () => {
    const f = await fixture([identity('matou-chatgpt/matou-opus'), assistant('gpt-6-astra')])
    expect(await f.read()).toBe(true)
  })
  it('preserves a newer official model and ignores synthetic/meta replies', async () => {
    const f = await fixture([identity('matou-chatgpt/matou-opus'), assistant('claude-sonnet-5'), assistant('<synthetic>'), { ...assistant('matou-chatgpt/matou-opus'), isMeta: true }])
    expect(await f.read()).toBe(false)
  })
  it('finds a model across chunk boundaries and large later records, using the canonical cwd bucket', async () => {
    const f = await fixture([identity('matou-antigravity/claude-opus-5'), { type: 'user', message: { content: '中文'.repeat(50000) } }])
    const alias = join(f.root, 'alias'); await symlink(f.cwd, alias)
    expect(await f.read(alias)).toBe(true)
    expect(await claudeResumeUsesSubscriptionModel(alias, '../unrelated', { CLAUDE_CONFIG_DIR: f.config })).toBe(false)
  })
  it('does not carry an older subscription model past a newer custom provider identity', async () => {
    const f = await fixture([assistant('matou-chatgpt/matou-opus'), identity('custom-gateway/model'), assistant('custom-upstream')])
    expect(await f.read()).toBe(false)
  })
})
