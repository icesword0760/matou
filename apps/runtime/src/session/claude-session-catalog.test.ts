import { appendFile, mkdir, mkdtemp, realpath, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { ClaudeSessionCatalog, encodeClaudeProjectPath } from './claude-session-catalog'

let root: string
let workspace: string
let catalog: ClaudeSessionCatalog

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-claude-catalog-'))
  workspace = join(root, 'current_workspace')
  await mkdir(workspace)
  catalog = new ClaudeSessionCatalog(join(root, 'projects'))
})

afterEach(() => undefined)

describe('ClaudeSessionCatalog', () => {
  it('discovers only recoverable sessions whose recorded cwd is the current workspace', async () => {
    await writeSession(workspace, 'current-session', [
      row('user', 'current-session', workspace, '2026-08-30T10:00:00.000Z', {
        role: 'user', content: '修复通知中心的聚合逻辑'
      }, { permissionMode: 'acceptEdits' }),
      row('assistant', 'current-session', workspace, '2026-08-30T10:01:00.000Z', {
        role: 'assistant', model: 'claude-opus-4-6', content: [{ type: 'text', text: '开始检查通知状态。' }]
      })
    ])
    await writeSession(join(root, 'other'), 'other-session', [
      row('user', 'other-session', join(root, 'other'), '2026-08-31T10:00:00.000Z', {
        role: 'user', content: '另一个项目'
      })
    ])

    const result = await catalog.list({ cwd: workspace, query: '' })

    expect(result.total).toBe(1)
    expect(result.sessions[0]).toMatchObject({
      providerSessionId: 'current-session',
      title: '修复通知中心的聚合逻辑',
      cwd: await realpath(workspace),
      model: 'claude-opus-4-6',
      permissionMode: 'acceptEdits',
      eventCount: 2,
      matchCount: 0
    })
  })

  it('searches message text and tool calls while returning exact preview event indexes', async () => {
    await writeSession(workspace, 'search-session', [
      row('user', 'search-session', workspace, '2026-08-30T10:00:00.000Z', {
        role: 'user', content: '检查卡片闪烁'
      }),
      row('assistant', 'search-session', workspace, '2026-08-30T10:01:00.000Z', {
        role: 'assistant', model: 'claude-sonnet-4-6', content: [
          { type: 'text', text: '先定位 hover 动画。' },
          { type: 'tool_use', name: 'Read', input: { file_path: '/tmp/SessionCanvas.tsx', pattern: 'hover width' } }
        ]
      }),
      row('user', 'search-session', workspace, '2026-08-30T10:02:00.000Z', {
        role: 'user', content: [{ type: 'tool_result', content: 'hover width transition found' }]
      })
    ])

    const result = await catalog.list({ cwd: workspace, query: 'hover width' })
    const detail = await catalog.detail({
      cwd: workspace, providerSessionId: 'search-session', query: 'hover width'
    })

    expect(result.sessions).toHaveLength(1)
    expect(result.sessions[0]?.matchCount).toBe(2)
    expect(result.sessions[0]?.hits.map(({ eventIndex }) => eventIndex)).toEqual([2, 3])
    expect(detail.events.filter(({ matched }) => matched).map(({ index }) => index)).toEqual([2, 3])
    expect(detail.events[1]).toMatchObject({ kind: 'tool', toolName: 'Read' })
  })

  it('keeps left-list metadata filtering separate from transcript content search', async () => {
    await writeSession(workspace, 'scope-session', [
      row('user', 'scope-session', workspace, '2026-08-30T10:00:00.000Z', {
        role: 'user', content: '固定会话标题'
      }),
      row('assistant', 'scope-session', workspace, '2026-08-30T10:01:00.000Z', {
        role: 'assistant', content: '只存在于正文里的独特词语'
      })
    ])

    const metadata = await catalog.list({
      cwd: workspace, query: '独特词语', searchScope: 'metadata'
    })
    const content = await catalog.search({
      cwd: workspace, providerSessionId: 'scope-session', query: '独特词语'
    })

    expect(metadata.sessions).toHaveLength(0)
    expect(content.hits).toHaveLength(1)
    expect(content.hits[0]?.eventIndex).toBe(2)
  })

  it('returns the latest event page while preserving the full event count', async () => {
    await writeSession(workspace, 'large-session', Array.from({ length: 400 }, (_, index) =>
      row('assistant', 'large-session', workspace, `2026-08-30T10:${String(index % 60).padStart(2, '0')}:00.000Z`, {
        role: 'assistant', content: `输出 ${index}`
      })))

    const detail = await catalog.detail({
      cwd: workspace, providerSessionId: 'large-session', query: '', limit: 200
    })

    expect(detail.eventCount).toBe(400)
    expect(detail.events).toHaveLength(200)
    expect(detail.events[0]?.index).toBe(201)
  })

  it('pages every recoverable session instead of applying a total-result cap', async () => {
    await Promise.all(Array.from({ length: 130 }, (_, index) => writeSession(
      workspace,
      `paged-${String(index).padStart(3, '0')}`,
      [row('user', `paged-${index}`, workspace,
        `2026-08-30T10:${String(index % 60).padStart(2, '0')}:00.000Z`, {
          role: 'user', content: `会话 ${index}`
        })]
    )))

    const first = await catalog.list({ cwd: workspace, query: '', offset: 0, limit: 50 })
    const last = await catalog.list({ cwd: workspace, query: '', offset: 100, limit: 50 })

    expect(first).toMatchObject({ total: 130, offset: 0, limit: 50, nextOffset: 50, hasMore: true })
    expect(first.sessions).toHaveLength(50)
    expect(last).toMatchObject({ total: 130, offset: 100, limit: 50, nextOffset: 130, hasMore: false })
    expect(last.sessions).toHaveLength(30)
    expect(new Set([...first.sessions, ...last.sessions].map(({ providerSessionId }) =>
      providerSessionId)).size).toBe(80)
  })

  it('opens the latest page, loads earlier events, and centers an arbitrary event', async () => {
    await writeSession(workspace, 'page-session', Array.from({ length: 400 }, (_, index) =>
      row('assistant', 'page-session', workspace, `2026-08-30T10:${String(index % 60).padStart(2, '0')}:00.000Z`, {
        role: 'assistant', content: `历史事件 ${index + 1}`
      })))

    const latest = await catalog.detail({
      cwd: workspace, providerSessionId: 'page-session', query: '', limit: 200
    })
    const earlier = await catalog.detail({
      cwd: workspace, providerSessionId: 'page-session', query: '',
      beforeEventIndex: latest.page.startEventIndex, limit: 200
    })
    const around = await catalog.detail({
      cwd: workspace, providerSessionId: 'page-session', query: '',
      aroundEventIndex: 151, limit: 40
    })

    expect(latest.events.map(({ index }) => index)).toEqual(Array.from({ length: 200 }, (_, index) => index + 201))
    expect(latest.page).toEqual({
      startEventIndex: 201, endEventIndex: 400, total: 400, hasEarlier: true, hasLater: false
    })
    expect(earlier.events.map(({ index }) => index)).toEqual(Array.from({ length: 200 }, (_, index) => index + 1))
    expect(earlier.page).toEqual({
      startEventIndex: 1, endEventIndex: 200, total: 400, hasEarlier: false, hasLater: true
    })
    expect(around.events[0]?.index).toBe(131)
    expect(around.events.at(-1)?.index).toBe(170)
    expect(around.page).toMatchObject({ total: 400, hasEarlier: true, hasLater: true })
  })

  it('searches the complete transcript with paged hits outside the latest event page', async () => {
    await writeSession(workspace, 'search-pages', Array.from({ length: 400 }, (_, index) =>
      row(index === 49 ? 'user' : 'assistant', 'search-pages', workspace,
        `2026-08-30T10:${String(index % 60).padStart(2, '0')}:00.000Z`, {
          role: index === 49 ? 'user' : 'assistant',
          content: index === 49 || index === 349 ? `远端命中 needle ${index + 1}` : `普通事件 ${index + 1}`
        })))

    const first = await catalog.search({
      cwd: workspace, providerSessionId: 'search-pages', query: 'NEEDLE', offset: 0, limit: 1
    })
    const second = await catalog.search({
      cwd: workspace, providerSessionId: 'search-pages', query: 'needle', offset: 1, limit: 1
    })

    expect(first).toMatchObject({ query: 'needle', total: 2, offset: 0, nextOffset: 1, hasMore: true })
    expect(first.hits.map(({ eventIndex }) => eventIndex)).toEqual([50])
    expect(second).toMatchObject({ total: 2, offset: 1, nextOffset: 2, hasMore: false })
    expect(second.hits.map(({ eventIndex }) => eventIndex)).toEqual([350])
  })

  it('finds a match at the end of a 24,000-event transcript without expanding the detail page', async () => {
    await writeSession(workspace, 'large-search-session', Array.from({ length: 24_000 }, (_, index) =>
      row('assistant', 'large-search-session', workspace, index, {
        role: 'assistant', content: index === 23_999 ? '最终跨页命中' : `普通事件 ${index + 1}`
      })))

    const detail = await catalog.detail({
      cwd: workspace, providerSessionId: 'large-search-session', query: '', limit: 200
    })
    const search = await catalog.search({
      cwd: workspace, providerSessionId: 'large-search-session', query: '最终跨页命中'
    })

    expect(detail).toMatchObject({ eventCount: 24_000, page: { startEventIndex: 23_801, total: 24_000 } })
    expect(detail.events).toHaveLength(200)
    expect(search).toMatchObject({ total: 1, hits: [{ eventIndex: 24_000 }] })
  })

  it('invalidates paged detail and search results after the transcript is appended', async () => {
    await writeSession(workspace, 'append-session', [
      row('user', 'append-session', workspace, '2026-08-30T10:00:00.000Z', {
        role: 'user', content: '初始内容'
      })
    ])
    const directory = join(root, 'projects', encodeClaudeProjectPath(workspace))
    const path = join(directory, 'append-session.jsonl')

    expect((await catalog.detail({
      cwd: workspace, providerSessionId: 'append-session', query: '', limit: 50
    })).eventCount).toBe(1)
    expect((await catalog.search({
      cwd: workspace, providerSessionId: 'append-session', query: '追加命中', offset: 0, limit: 20
    })).total).toBe(0)

    await appendFile(path, `\n${JSON.stringify(row(
      'assistant', 'append-session', workspace, '2026-08-30T10:01:00.000Z', {
        role: 'assistant', content: [{ type: 'tool_result', content: 'Unicode 追加命中' }]
      }
    ))}`)

    const detail = await catalog.detail({
      cwd: workspace, providerSessionId: 'append-session', query: '', limit: 50
    })
    const search = await catalog.search({
      cwd: workspace, providerSessionId: 'append-session', query: '追加命中', offset: 0, limit: 20
    })
    expect(detail.eventCount).toBe(2)
    expect(detail.events.at(-1)?.text).toContain('Unicode 追加命中')
    expect(search.hits).toEqual([expect.objectContaining({ eventIndex: 2, kind: 'tool' })])
  })

  it('uses the latest explicit permission mode and skips malformed transcript lines', async () => {
    const directory = join(root, 'projects', encodeClaudeProjectPath(workspace))
    await mkdir(directory, { recursive: true })
    await writeFile(join(directory, 'permission-session.jsonl'), [
      JSON.stringify(row('user', 'permission-session', workspace, '2026-08-30T10:00:00.000Z', {
        role: 'user', content: '开放权限继续实现'
      }, { permissionMode: 'default' })),
      '{broken json',
      JSON.stringify({
        type: 'permission-mode', sessionId: 'permission-session', cwd: workspace,
        permissionMode: 'bypassPermissions', timestamp: '2026-08-30T10:01:00.000Z'
      })
    ].join('\n'))

    const detail = await catalog.detail({
      cwd: workspace, providerSessionId: 'permission-session', query: ''
    })

    expect(detail.permissionMode).toBe('bypassPermissions')
    expect(detail.title).toBe('开放权限继续实现')
  })

  it('preserves the current Claude Code auto permission mode', async () => {
    await writeSession(workspace, 'auto-session', [
      row('user', 'auto-session', workspace, '2026-08-30T10:00:00.000Z', {
        role: 'user', content: '继续处理自动权限会话'
      }, { permissionMode: 'auto' })
    ])

    const detail = await catalog.detail({
      cwd: workspace, providerSessionId: 'auto-session', query: ''
    })

    expect(detail.permissionMode).toBe('auto')
  })

  it('uses the latest Claude Code ai-title instead of synthesizing one from the first prompt', async () => {
    await writeSession(workspace, 'titled-session', [
      row('user', 'titled-session', workspace, '2026-08-30T10:00:00.000Z', {
        role: 'user', content: '这里是一条很长的原始需求，不适合作为卡片标题直接显示'
      }),
      { type: 'ai-title', sessionId: 'titled-session', aiTitle: '第一次生成的标题' },
      row('assistant', 'titled-session', workspace, '2026-08-30T10:01:00.000Z', {
        role: 'assistant', content: '已经完成第一轮处理。'
      }),
      { type: 'ai-title', sessionId: 'titled-session', aiTitle: '修复卡片标题同步' }
    ])

    const detail = await catalog.detail({
      cwd: workspace, providerSessionId: 'titled-session', query: ''
    })

    expect(detail.title).toBe('修复卡片标题同步')
    await expect(catalog.autoTitle({
      cwd: workspace, providerSessionId: 'titled-session'
    })).resolves.toBe('修复卡片标题同步')
  })
})

async function writeSession(cwd: string, id: string, rows: unknown[]): Promise<void> {
  const directory = join(root, 'projects', encodeClaudeProjectPath(cwd))
  await mkdir(directory, { recursive: true })
  await writeFile(join(directory, `${id}.jsonl`), rows.map((value) => JSON.stringify(value)).join('\n'))
}

function row(
  type: string,
  sessionId: string,
  cwd: string,
  timestamp: string | number,
  message: unknown,
  extra: Record<string, unknown> = {}
): Record<string, unknown> {
  return { type, sessionId, cwd, timestamp, message, ...extra }
}
