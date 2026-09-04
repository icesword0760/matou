import { mkdir, mkdtemp, realpath, writeFile } from 'node:fs/promises'
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
    const content = await catalog.detail({
      cwd: workspace, providerSessionId: 'scope-session', query: '独特词语', previewLimit: 240
    })

    expect(metadata.sessions).toHaveLength(0)
    expect(content.events).toHaveLength(1)
    expect(content.events[0]?.matched).toBe(true)
  })

  it('bounds a large detail preview while preserving the full event count', async () => {
    await writeSession(workspace, 'large-session', Array.from({ length: 400 }, (_, index) =>
      row('assistant', 'large-session', workspace, `2026-08-30T10:${String(index % 60).padStart(2, '0')}:00.000Z`, {
        role: 'assistant', content: `输出 ${index}`
      })))

    const detail = await catalog.detail({
      cwd: workspace, providerSessionId: 'large-session', query: '', previewLimit: 240
    })

    expect(detail.eventCount).toBe(400)
    expect(detail.events).toHaveLength(240)
    expect(detail.events[0]?.index).toBe(161)
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
  timestamp: string,
  message: unknown,
  extra: Record<string, unknown> = {}
): Record<string, unknown> {
  return { type, sessionId, cwd, timestamp, message, ...extra }
}
