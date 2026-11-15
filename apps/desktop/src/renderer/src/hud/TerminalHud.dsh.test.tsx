// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { TerminalHud } from './TerminalHud'
import type { SessionHudView } from '../hierarchy/hierarchy-types'

afterEach(() => cleanup())

describe('TerminalHud DSH', () => {
  it('shows the authoritative native mode as a non-interactive badge without a switch menu', () => {
    const onPermissionMode = vi.fn()
    const { container } = render(<TerminalHud hud={dsh({
      providerPermissionLabel: '只读',
      model: 'deepseek-v3.2',
      effort: 'high',
      contextPercent: 42,
      cwd: '/Users/demo/project',
      gitBranch: 'main',
      gitDirty: true
    })} onPermissionMode={onPermissionMode} />)

    const badge = container.querySelector('.status-perm-badge')
    expect(badge?.tagName).toBe('SPAN')
    expect(badge?.textContent).toBe('只读')
    expect(badge?.getAttribute('aria-label')).toBe('只读')
    expect(screen.queryByRole('button', { name: /当前权限模式/ })).toBeNull()
    fireEvent.click(badge!)
    expect(screen.queryByRole('menu')).toBeNull()
    expect(onPermissionMode).not.toHaveBeenCalled()
    expect(screen.getByText('deepseek-v3.2 (high)')).toBeTruthy()
    expect(screen.getByText('42%')).toBeTruthy()
    expect(screen.getByText('~/project')).toBeTruthy()
    expect(screen.getByText('main*')).toBeTruthy()
  })

  it('hides the mode, model and context instead of inventing a default or zero', () => {
    const { container } = render(<TerminalHud hud={dsh({
      permissionMode: 'default',
      modelStrategy: 'opusplan',
      contextWindowSize: 200_000
    })} />)

    expect(container.querySelector('.status-perm-badge')).toBeNull()
    expect(screen.queryByText('Opus 4.6')).toBeNull()
    expect(screen.queryByText('等待真实用量')).toBeNull()
    expect(screen.queryByText(/^0%$/)).toBeNull()
    expect(container.querySelector('.status-context-ring')).toBeNull()
  })

  it('keeps account, quota and team fields out of the DSH HUD', () => {
    const { container } = render(<TerminalHud hud={dsh({
      providerPermissionLabel: '只读',
      teamRole: 'Leader',
      teamStatus: 'running',
      cwd: '/Users/demo/project',
      gitBranch: 'main',
      defaultAccountUsage: true,
      usageWindows: [{ label: 'Weekly', percent: 8, resetsAt: Date.now() + 3_600_000 }],
      allUsageWindows: [{ label: 'Weekly', percent: 8, resetsAt: Date.now() + 3_600_000 }]
    })} />)

    expect(screen.queryByText('Leader')).toBeNull()
    expect(screen.queryByText('Weekly')).toBeNull()
    expect(screen.queryByText('8%')).toBeNull()
    expect(container.querySelector('.status-usage')).toBeNull()
    expect(container.querySelector('.team-role-badge')).toBeNull()
    expect(container.querySelector('.status-divider')).toBeNull()
  })

  it('reuses existing tool rendering and does not paint a zero when no tools arrived', () => {
    const view = render(<TerminalHud hud={dsh({
      providerPermissionLabel: '只读',
      runningTools: [{ name: 'read', target: 'src/App.tsx' }],
      toolCounts: [{ name: 'bash', count: 2 }]
    })} />)
    expect(screen.getByText('3 Tools')).toBeTruthy()

    view.rerender(<TerminalHud hud={dsh({ providerPermissionLabel: '只读' })} />)
    expect(screen.queryByText(/Tools/)).toBeNull()
  })

  it('shows a project instruction entry only when the HUD supplies an authoritative label', () => {
    const view = render(<TerminalHud hud={dsh({
      providerPermissionLabel: '只读',
      instructionFileLabel: 'AGENTS.md',
      configCounts: { instructionFiles: 0, projectInstructionFileExists: false, mcpServers: 0, hooks: 0 }
    })} runtimeClient={{ request: vi.fn() }} />)
    expect(screen.getByRole('button', { name: 'AGENTS.md' })).toBeTruthy()

    view.rerender(<TerminalHud hud={dsh({
      providerPermissionLabel: '只读',
      configCounts: { instructionFiles: 1, projectInstructionFileExists: true, mcpServers: 0, hooks: 0 }
    })} runtimeClient={{ request: vi.fn() }} />)
    expect(screen.queryByRole('button', { name: 'AGENTS.md' })).toBeNull()
  })
})

function dsh(patch: Partial<SessionHudView> = {}): SessionHudView {
  return {
    sessionId: 'dsh-session', mode: 'agent', provider: 'dsh', startedAt: Date.now(),
    ...patch
  }
}
