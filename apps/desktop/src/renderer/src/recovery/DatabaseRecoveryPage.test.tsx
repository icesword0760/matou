// @vitest-environment jsdom
import { cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { RuntimeLifecyclePresentation } from '../../../shared/desktop-api'
import { DatabaseRecoveryPage } from './DatabaseRecoveryPage'

afterEach(cleanup)

describe('database recovery page', () => {
  it('shows at most seven newest valid backups and selects the latest by default', () => {
    render(<DatabaseRecoveryPage state={state(9)} actions={actions()} />)

    const choices = screen.getAllByRole('radio')
    expect(choices).toHaveLength(7)
    expect((choices[0] as HTMLInputElement).checked).toBe(true)
    expect(screen.queryByText('backup-1')).toBeNull()
    expect(screen.getByText('backup-9')).toBeTruthy()
  })

  it('prevents duplicate restore submissions while the selected backup is restoring', async () => {
    let finish!: () => void
    const restore = vi.fn(() => new Promise<void>((resolve) => { finish = resolve }))
    render(<DatabaseRecoveryPage state={state(2)} actions={{ ...actions(), restore }} />)
    const button = screen.getByRole('button', { name: '恢复所选备份' })

    await userEvent.setup().click(button)
    await userEvent.setup().click(button)
    expect(restore).toHaveBeenCalledTimes(1)
    expect(button.hasAttribute('disabled')).toBe(true)
    finish()
  })

  it('exports evidence, retries opening, and displays the original failure on the page', async () => {
    const exportBundle = vi.fn().mockResolvedValue({ exportedPath: '/exports/recovery-1' })
    const retry = vi.fn().mockRejectedValue(new Error('重新检查仍发现损坏'))
    render(<DatabaseRecoveryPage state={state(1)} actions={{ ...actions(), exportBundle, retry }} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '导出恢复资料' }))
    expect(await screen.findByText(/\/exports\/recovery-1/)).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '重新检查数据库' }))
    expect((await screen.findByRole('alert')).textContent).toContain('重新检查仍发现损坏')
  })

  it('requires an explicit second confirmation before starting with an empty database', async () => {
    const startEmpty = vi.fn().mockResolvedValue(undefined)
    render(<DatabaseRecoveryPage state={state(0)} actions={{ ...actions(), startEmpty }} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '创建全新空数据库' }))
    expect(startEmpty).not.toHaveBeenCalled()
    const dialog = screen.getByRole('dialog', { name: '确认创建全新空数据库' })
    await userEvent.setup().click(within(dialog).getByRole('button', { name: '确认创建空数据库' }))
    expect(startEmpty).toHaveBeenCalledOnce()
  })

  it('moves focus into the empty-database dialog, traps Tab, and restores focus on Escape', async () => {
    const user = userEvent.setup()
    render(<DatabaseRecoveryPage state={state(0)} actions={actions()} />)
    const opener = screen.getByRole('button', { name: '创建全新空数据库' })

    await user.click(opener)
    const dialog = screen.getByRole('dialog', { name: '确认创建全新空数据库' })
    const back = within(dialog).getByRole('button', { name: '返回' })
    const confirm = within(dialog).getByRole('button', { name: '确认创建空数据库' })
    expect(document.activeElement).toBe(back)

    await user.keyboard('{Shift>}{Tab}{/Shift}')
    expect(document.activeElement).toBe(confirm)
    await user.keyboard('{Tab}')
    expect(document.activeElement).toBe(back)
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.activeElement).toBe(opener)
  })

  it('states that ownership recovery preserves the original database', () => {
    const value = state(0)
    value.recovery!.reason = 'ownership-recovery-required'
    value.recovery!.quarantinedPath = value.recovery!.durableDatabasePath
    render(<DatabaseRecoveryPage state={value} actions={actions()} />)
    expect(screen.getByText(/原数据库仍保留在原位置/)).toBeTruthy()
    expect(screen.queryByText(/主数据库已损坏/)).toBeNull()
  })
})

function state(count: number): RuntimeLifecyclePresentation {
  return {
    snapshot: {
      recoveryId: 'recovery-ui', revision: 1, mode: 'recovery-required',
      stage: 'opening-database', completed: 0, total: 1, failures: []
    },
    recovery: {
      reason: 'physical-corruption',
      durableDatabasePath: '/data/matou.sqlite',
      quarantinedPath: '/data/matou.sqlite.corrupt-1',
      backups: Array.from({ length: count }, (_, index) => ({
        id: `backup-${index + 1}`,
        createdAt: index + 1,
        reason: 'clean-exit' as const,
        schemaVersion: 21,
        size: 1024,
        sha256: String(index + 1).padStart(64, '0')
      }))
    }
  }
}

function actions() {
  return {
    restore: vi.fn().mockResolvedValue(undefined),
    exportBundle: vi.fn().mockResolvedValue({}),
    retry: vi.fn().mockResolvedValue(undefined),
    startEmpty: vi.fn().mockResolvedValue(undefined)
  }
}
