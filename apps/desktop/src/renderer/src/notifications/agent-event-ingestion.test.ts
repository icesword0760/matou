import { describe, expect, it, vi } from 'vitest'

import type { DomainEventWireEnvelope } from '@matou/contracts'

import type { HierarchyProjection } from '../hierarchy/hierarchy-types'
import { AgentNotificationStore } from './AgentNotificationStore'
import { ingestAgentNotification } from './agent-event-ingestion'

describe('ingestAgentNotification', () => {
  it('maps the semantic Agent event back to all four visible hierarchy levels', () => {
    const store = new AgentNotificationStore({ now: () => 20, playSound: vi.fn() })

    expect(ingestAgentNotification(domainEvent(), projection(), 'session-other', store)).toBe(true)

    expect(store.snapshot().notifications[0]).toMatchObject({
      eventId: 'provider-event', eventType: 'completed', workspaceId: 'workspace-1',
      taskId: 'task-1', sceneId: 'scene-1', sessionId: 'session-1', read: false
    })
  })

  it('marks a focused Session event read without creating a second visible indicator', () => {
    const playSound = vi.fn()
    const store = new AgentNotificationStore({ now: () => 20, playSound })

    ingestAgentNotification(domainEvent(), projection(), 'session-1', store)

    expect(store.snapshot().notifications[0]?.read).toBe(true)
    expect(store.sessionHasVisibleIndicator('session-1')).toBe(false)
    expect(playSound).not.toHaveBeenCalled()
  })

  it('keeps an event with deleted hierarchy as a visible unknown-location entry', () => {
    const store = new AgentNotificationStore({ now: () => 20 })
    const event = domainEvent()
    delete event.workspaceId
    delete event.taskId
    delete event.sessionId
    event.aggregateId = 'deleted-session'
    event.payload = {
      targetSessionId: 'deleted-session', provider: 'claude-code', runId: 'run-1',
      event: { eventType: 'attention', title: 'Claude Code', subtitle: 'Attention', body: '需要处理', sound: true }
    }

    expect(ingestAgentNotification(event, projection(), undefined, store)).toBe(true)
    expect(store.snapshot().notifications[0]).toMatchObject({
      workspaceId: null, taskId: null, sceneId: null, sessionId: 'deleted-session'
    })
  })

  it('ignores ordinary domain changes instead of turning them into user notifications', () => {
    const store = new AgentNotificationStore({ now: () => 20 })
    const event = domainEvent()
    event.eventType = 'session.updated'

    expect(ingestAgentNotification(event, projection(), undefined, store)).toBe(false)
    expect(store.snapshot().notifications).toHaveLength(0)
  })

  it('discards legacy recovery notifications because recovery state is shown on the card', () => {
    let now = 20
    const store = new AgentNotificationStore({ now: () => now })
    const failed = domainEvent()
    failed.eventId = 'restore-failed'
    failed.payload = {
      targetSessionId: 'session-1', provider: 'claude-code', runId: 'restore:binding-1',
      event: {
        operation: 'upsert', replacementKey: 'provider-restore:session-1',
        eventType: 'error', title: 'Claude Code 恢复失败', body: '会话不存在', sound: true
      }
    }
    ingestAgentNotification(failed, projection(), undefined, store)
    expect(store.snapshot().notifications).toHaveLength(0)

    now = 21
    const retrying = domainEvent()
    retrying.eventId = 'restore-retrying'
    retrying.payload = {
      targetSessionId: 'session-1', provider: 'claude-code', runId: 'restore:binding-1',
      event: {
        operation: 'upsert', replacementKey: 'provider-restore:session-1',
        eventType: 'attention', title: '正在恢复 Claude Code', sound: false
      }
    }
    ingestAgentNotification(retrying, projection(), undefined, store)
    expect(store.snapshot().notifications).toHaveLength(0)

    const dismissed = domainEvent()
    dismissed.eventId = 'restore-succeeded'
    dismissed.payload = {
      targetSessionId: 'session-1', provider: 'claude-code', runId: 'restore:binding-1',
      event: { operation: 'dismiss', replacementKey: 'provider-restore:session-1' }
    }
    ingestAgentNotification(dismissed, projection(), undefined, store)
    expect(store.snapshot().notifications).toHaveLength(0)
  })
})

function domainEvent(): DomainEventWireEnvelope {
  return {
    sequence: 8, eventId: 'provider-event', eventType: 'agent.notification',
    aggregateType: 'session', aggregateId: 'session-1', workspaceId: 'workspace-1',
    taskId: 'task-1', sessionId: 'session-1', schemaVersion: 1,
    commandId: 'publish-provider-event', occurredAt: 10,
    payload: {
      targetSessionId: 'session-1', provider: 'claude-code', runId: 'run-1',
      event: {
        eventType: 'completed', title: 'Claude Code', subtitle: 'Completed',
        body: '任务完成', sound: true
      }
    }
  }
}

function projection(): HierarchyProjection {
  return {
    windowId: 'window-1',
    workspaces: [{ id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp' }],
    tasks: [{ id: 'task-1', workspaceId: 'workspace-1', title: 'Task' }],
    scenes: [{ id: 'scene-1', taskId: 'task-1', name: 'Scene' }],
    sessions: [{ id: 'session-1', taskId: 'task-1', title: 'Claude' }],
    sceneSnapshots: [{
      scene: { id: 'scene-1', taskId: 'task-1', name: 'Scene' },
      nodes: [], windows: [], mounts: [{ id: 'mount-1', sceneId: 'scene-1', sessionId: 'session-1' }]
    }],
    pathStates: [], taskPlacements: [],
    navigation: {
      windowId: 'window-1', activeWorkspaceId: 'workspace-1',
      taskByWorkspace: { 'workspace-1': 'task-1' }, sceneByTask: { 'task-1': 'scene-1' },
      sessionByScene: { 'scene-1': 'session-1' }
    }
  }
}
