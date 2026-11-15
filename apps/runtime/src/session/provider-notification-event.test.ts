import { describe, expect, it } from 'vitest'

import { toProviderNotificationEvent } from './provider-notification-event'

describe('toProviderNotificationEvent', () => {
  it('uses the shared completion and approval notifications for the observed Codex payloads', () => {
    expect(toProviderNotificationEvent({
      hook_event_name: 'PermissionRequest', session_id: 'codex-main',
      model: 'gpt-6-astra', permission_mode: 'default', tool_name: 'Bash',
      tool_input: { command: "python3 -c 'print(2)'", description: 'Approve this harmless command?' }
    }, 'codex')).toEqual({
      eventType: 'permission', title: 'Codex', subtitle: 'Permission',
      body: "Bash: python3 -c 'print(2)'", sound: true, cooldownKey: 'Notification'
    })
    expect(toProviderNotificationEvent({
      hook_event_name: 'Stop', cwd: '/tmp/probe', stop_hook_active: false,
      last_assistant_message: 'The command exited successfully.\nOutput: 2'
    }, 'codex')).toEqual({
      eventType: 'completed', title: 'Codex', subtitle: 'Completed in probe',
      body: 'The command exited successfully. Output: 2', sound: true, cooldownKey: 'Stop'
    })
  })

  it('does not mistake the observed failed Codex command for a failed model turn or rewrite its answer as success', () => {
    expect(toProviderNotificationEvent({
      hook_event_name: 'PostToolUse', tool_name: 'Bash',
      tool_response: 'zsh:1: command not found: matou_probe_missing_command\n'
    }, 'codex')).toBeNull()
    // This is a normally completed model reply explaining a failed command.
    // The real Stop contains no structured turn-error or cancellation marker.
    expect(toProviderNotificationEvent({
      hook_event_name: 'Stop', stop_hook_active: false,
      last_assistant_message: 'Ran once. Exit code: `127`.\nCommand not found.'
    }, 'codex')).toMatchObject({
      eventType: 'completed', title: 'Codex', body: 'Ran once. Exit code: `127`. Command not found.'
    })
    expect(toProviderNotificationEvent({ hook_event_name: 'UserPromptSubmit' }, 'codex')).toBeNull()
    expect(toProviderNotificationEvent({ hook_event_name: 'SessionEnd' }, 'codex')).toBeNull()
  })

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

  it('keeps failure wording as attention while classifying completion and waiting', () => {
    expect(toProviderNotificationEvent({ hook_event_name: 'Notification', message: 'Tool failed' })).toMatchObject({
      eventType: 'attention', subtitle: 'Attention'
    })
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

  it('maps Antigravity Stop, ERROR, USER_CANCELED, and tool_use transitions with Antigravity title', () => {
    expect(toProviderNotificationEvent({
      hookEventName: 'Stop',
      cwd: '/Users/demo/projects/matou',
      terminationReason: 'NO_TOOL_CALL',
      error: ''
    }, 'antigravity')).toEqual({
      eventType: 'completed',
      title: 'Antigravity',
      subtitle: 'Completed in matou',
      body: 'Antigravity session completed in matou',
      sound: true,
      cooldownKey: 'Stop'
    })

    expect(toProviderNotificationEvent({
      hookEventName: 'Stop',
      cwd: '/Users/demo/projects/matou',
      terminationReason: 'ERROR',
      error: 'API request failed'
    }, 'antigravity')).toEqual({
      eventType: 'error',
      title: 'Antigravity',
      subtitle: 'Error in matou',
      body: 'API request failed',
      sound: true,
      cooldownKey: 'Stop'
    })

    expect(toProviderNotificationEvent({
      hookEventName: 'Stop',
      terminationReason: 'USER_CANCELED',
      error: ''
    }, 'antigravity')).toBeNull()

    expect(toProviderNotificationEvent({
      agent_state: 'tool_use'
    }, 'antigravity', { previousAgentState: 'working' })).toMatchObject({
      eventType: 'permission',
      title: 'Antigravity',
      subtitle: 'Permission'
    })

    expect(toProviderNotificationEvent({
      agent_state: 'tool_use'
    }, 'antigravity', { previousAgentState: 'tool_use' })).toBeNull()
  })
})
