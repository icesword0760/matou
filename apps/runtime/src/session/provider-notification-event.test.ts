import { describe, expect, it } from 'vitest'

import { toProviderNotificationEvent } from './provider-notification-event'

describe('toProviderNotificationEvent', () => {
  it('turns Stop into the latest Claude answer and a Completed project subtitle', () => {
    expect(toProviderNotificationEvent({
      hook_event_name: 'Stop', cwd: '/Users/demo/projects/matou',
      last_assistant_message: '  完成了。\n\n请检查结果。  '
    })).toEqual({
      eventType: 'completed', title: 'Claude Code', subtitle: 'Completed in matou',
      body: '完成了。 请检查结果。', sound: true, cooldownKey: 'Stop'
    })
  })

  it('classifies a permission hook without rewriting its original message', () => {
    expect(toProviderNotificationEvent({
      hook_event_name: 'Notification', message: 'Permission required to run the command'
    })).toEqual({
      eventType: 'permission', title: 'Claude Code', subtitle: 'Permission',
      body: 'Permission required to run the command', sound: true, cooldownKey: 'Notification'
    })
  })

  it('maps the provider PermissionRequest hook even when no Notification hook is emitted', () => {
    expect(toProviderNotificationEvent({
      hook_event_name: 'PermissionRequest', tool_name: 'Write',
      tool_input: { file_path: '/tmp/project/file.ts' }
    })).toMatchObject({
      eventType: 'permission', subtitle: 'Permission', body: 'Write: /tmp/project/file.ts'
    })
  })

  it('classifies errors, completion, waiting, and generic attention in reference product order', () => {
    expect(toProviderNotificationEvent({ hook_event_name: 'Notification', message: 'Tool failed' })?.eventType).toBe('error')
    expect(toProviderNotificationEvent({ hook_event_name: 'Notification', message: 'Task done' })?.eventType).toBe('completed')
    expect(toProviderNotificationEvent({ hook_event_name: 'Notification', message: 'Waiting for input' })?.eventType).toBe('waiting')
    expect(toProviderNotificationEvent({ hook_event_name: 'Notification', message: 'Please review this' })?.eventType).toBe('attention')
  })

  it('does not create a second notification for SessionEnd or operational hooks', () => {
    expect(toProviderNotificationEvent({ hook_event_name: 'SessionEnd' })).toBeNull()
    expect(toProviderNotificationEvent({ hook_event_name: 'PreToolUse' })).toBeNull()
    expect(toProviderNotificationEvent({ hook_event_name: 'UserPromptSubmit' })).toBeNull()
  })

  it('keeps a usable fallback when a supported hook has no message', () => {
    expect(toProviderNotificationEvent({ hook_event_name: 'Notification' })).toMatchObject({
      eventType: 'attention', subtitle: 'Attention', body: 'Claude needs your attention'
    })
    expect(toProviderNotificationEvent({ hook_event_name: 'Stop', cwd: '/tmp/demo' })).toMatchObject({
      eventType: 'completed', subtitle: 'Completed in demo', body: 'Claude session completed in demo'
    })
  })

  it('bounds provider content to a compact notification-safe single line', () => {
    const body = toProviderNotificationEvent({
      hook_event_name: 'Notification', message: `Waiting ${'x'.repeat(300)}`
    })?.body
    expect(body).toHaveLength(180)
    expect(body?.endsWith('…')).toBe(true)
  })
})
