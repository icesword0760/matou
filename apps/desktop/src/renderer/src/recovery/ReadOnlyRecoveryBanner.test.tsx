// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ReadOnlyRecoveryBanner } from './ReadOnlyRecoveryBanner'

afterEach(cleanup)

describe('ReadOnlyRecoveryBanner', () => {
  it('offers a direct search entry when a terminal history is present', async () => {
    const onSearch = vi.fn()
    render(<ReadOnlyRecoveryBanner onSearch={onSearch} exportBundle={vi.fn()} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '搜索当前终端' }))

    expect(onSearch).toHaveBeenCalledTimes(1)
  })

  it('keeps the recovery explanation visible and exports a browsable database bundle', async () => {
    const exportBundle = vi.fn().mockResolvedValue({ exportedPath: '/tmp/matou-export' })
    render(<ReadOnlyRecoveryBanner exportBundle={exportBundle} />)

    expect(screen.getByRole('status').textContent).toContain('数据库处于只读恢复模式')
    await userEvent.setup().click(screen.getByRole('button', { name: '导出数据库资料' }))

    expect(exportBundle).toHaveBeenCalledTimes(1)
    expect(await screen.findByText('数据库资料已导出到 /tmp/matou-export')).toBeTruthy()
  })

  it('keeps the export entry available after a failed attempt', async () => {
    const exportBundle = vi.fn().mockRejectedValue(new Error('目标目录不可写'))
    render(<ReadOnlyRecoveryBanner exportBundle={exportBundle} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '导出数据库资料' }))

    expect((await screen.findByRole('alert')).textContent).toContain('导出失败：目标目录不可写')
    expect(screen.getByRole('button', { name: '导出数据库资料' })).toHaveProperty('disabled', false)
  })
})
