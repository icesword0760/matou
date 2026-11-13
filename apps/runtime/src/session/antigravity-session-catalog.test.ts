import { join } from 'node:path'

import { mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'

import { beforeEach, describe, expect, it } from 'vitest'

import {
  AntigravitySessionCatalog,
  antigravityMetadataPath,
  antigravityTranscriptPath,
  latestAntigravityAutoTitle
} from './antigravity-session-catalog'

let root: string
let antigravityRoot: string
let workspace: string
let catalog: AntigravitySessionCatalog

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-agy-catalog-'))
  antigravityRoot = join(root, 'antigravity-cli')
  workspace = join(root, 'workspace')
  await mkdir(join(antigravityRoot, 'cache'), { recursive: true })
  await mkdir(workspace, { recursive: true })
  catalog = new AntigravitySessionCatalog(antigravityRoot)
})

describe('AntigravitySessionCatalog', () => {
  it('filters conversations by AppDataDir=antigravity-cli, non-internal, and matching WorkspaceURIs', async () => {
    const otherWorkspace = join(root, 'other')
    await mkdir(otherWorkspace, { recursive: true })
    await writeFile(antigravityMetadataPath(antigravityRoot), JSON.stringify({
      conversations: {
        'agy-1': {
          summary: {
            ID: 'agy-1',
            Title: '梳理项目规则',
            Preview: '帮我看一下',
            NumSteps: 12,
            UpdatedAt: '2026-11-12T10:00:00Z',
            WorkspaceURIs: [`file://${workspace}`],
            AppDataDir: 'antigravity-cli',
            Internal: false
          },
          is_internal: false
        },
        'agy-preview-only': {
          summary: {
            ID: 'agy-preview-only',
            Title: '',
            Preview: '  这是一条很长的首句预览内容   用来替代空标题显示在会话列表里  '.repeat(4),
            NumSteps: 4,
            UpdatedAt: '2026-11-12T11:00:00Z',
            WorkspaceURIs: [`file://${workspace}`],
            AppDataDir: 'antigravity-cli'
          }
        },
        'agy-internal': {
          summary: {
            ID: 'agy-internal',
            Title: 'Internal',
            NumSteps: 2,
            UpdatedAt: '2026-11-12T12:00:00Z',
            WorkspaceURIs: [`file://${workspace}`],
            AppDataDir: 'antigravity-cli',
            Internal: true
          },
          is_internal: true
        },
        'agy-wrong-appdir': {
          summary: {
            ID: 'agy-wrong-appdir',
            Title: 'IDE Session',
            NumSteps: 8,
            UpdatedAt: '2026-11-12T12:00:00Z',
            WorkspaceURIs: [`file://${workspace}`],
            AppDataDir: 'antigravity'
          }
        },
        'agy-other-workspace': {
          summary: {
            ID: 'agy-other-workspace',
            Title: 'Other Workspace',
            NumSteps: 5,
            UpdatedAt: '2026-11-12T12:00:00Z',
            WorkspaceURIs: [`file://${otherWorkspace}`],
            AppDataDir: 'antigravity-cli'
          }
        }
      }
    }))

    const list = await catalog.list({ cwd: workspace, query: '', searchScope: 'metadata' })
    expect(list.provider).toBe('antigravity')
    expect(list.total).toBe(2)
    expect(list.sessions.map((item) => item.providerSessionId)).toEqual(['agy-preview-only', 'agy-1'])
    expect(list.sessions[0]).toMatchObject({
      provider: 'antigravity',
      eventCount: 4,
      permissionMode: 'default'
    })
    expect(list.sessions[0]!.title.length).toBeLessThanOrEqual(72)
    expect(list.sessions[0]!.title.endsWith('…')).toBe(true)
    expect(list.sessions[1]).toMatchObject({
      title: '梳理项目规则',
      eventCount: 12
    })
    expect(await catalog.autoTitle({ cwd: workspace, providerSessionId: 'agy-preview-only' })).toBeUndefined()
    expect(await catalog.autoTitle({ cwd: workspace, providerSessionId: 'agy-1' })).toBe('梳理项目规则')
  })

  it('reads transcript.jsonl events for detail pagination and content search', async () => {
    await writeFile(antigravityMetadataPath(antigravityRoot), JSON.stringify({
      conversations: {
        'agy-transcript': {
          summary: {
            ID: 'agy-transcript',
            Title: '测试对话历史',
            NumSteps: 3,
            UpdatedAt: '2026-11-12T10:00:00Z',
            WorkspaceURIs: [`file://${workspace}`],
            AppDataDir: 'antigravity-cli'
          }
        }
      }
    }))
    const transcriptFile = antigravityTranscriptPath(antigravityRoot, 'agy-transcript')
    await mkdir(join(transcriptFile, '..'), { recursive: true })
    await writeFile(transcriptFile, [
      JSON.stringify({
        step_index: 1,
        source: 'USER_EXPLICIT',
        type: 'USER_INPUT',
        created_at: '2026-11-12T10:00:00Z',
        content: '检查打包产物里的 antigravity-system-message.md'
      }),
      JSON.stringify({
        step_index: 2,
        source: 'MODEL',
        type: 'PLANNER_RESPONSE',
        created_at: '2026-11-12T10:00:02Z',
        content: '我先读取文件内容。',
        tool_calls: [{ id: 'tc-1', name: 'read_file', arguments: '{"Path":"/repo/dist/control-assets"}' }]
      }),
      JSON.stringify({
        step_index: 3,
        source: 'MODEL',
        type: 'PLANNER_RESPONSE',
        created_at: '2026-11-12T10:00:04Z',
        content: '文件已确认存在，可以继续下一步。'
      })
    ].join('\n'))

    const detail = await catalog.detail({
      cwd: workspace,
      providerSessionId: 'agy-transcript',
      query: 'control-assets'
    })
    expect(detail.provider).toBe('antigravity')
    expect(detail.events).toHaveLength(3)
    expect(detail.events[0]).toMatchObject({ index: 1, kind: 'user', role: 'user', matched: false })
    expect(detail.events[1]).toMatchObject({ index: 2, kind: 'tool', role: 'assistant', toolName: 'read_file', matched: true })
    expect(detail.events[2]).toMatchObject({ index: 3, kind: 'assistant', role: 'assistant', matched: false })
    expect(detail.matchCount).toBe(1)

    const search = await catalog.search({
      cwd: workspace,
      providerSessionId: 'agy-transcript',
      query: '确认存在'
    })
    expect(search.total).toBe(1)
    expect(search.hits[0]).toMatchObject({ eventIndex: 3, kind: 'assistant' })

    const earlier = await catalog.detail({
      cwd: workspace,
      providerSessionId: 'agy-transcript',
      query: '',
      beforeEventIndex: 3,
      limit: 1
    })
    expect(earlier.events.map((e) => e.index)).toEqual([2])
    expect(earlier.page).toMatchObject({
      startEventIndex: 2,
      endEventIndex: 2,
      total: 3,
      hasEarlier: true,
      hasLater: true
    })
  })

  it('extracts auto title only when Title is non-empty and ignores Preview', async () => {
    const metadataFile = antigravityMetadataPath(antigravityRoot)
    await writeFile(metadataFile, JSON.stringify({
      conversations: {
        'agy-no-title': { summary: { ID: 'agy-no-title', Title: '   ', Preview: 'Only preview' } },
        'agy-titled': { summary: { ID: 'agy-titled', Title: '  自动生成标题  ', Preview: 'Preview' } }
      }
    }))

    expect(await latestAntigravityAutoTitle(metadataFile, 'agy-no-title')).toBeUndefined()
    expect(await latestAntigravityAutoTitle(metadataFile, 'agy-titled')).toBe('自动生成标题')
  })
})
