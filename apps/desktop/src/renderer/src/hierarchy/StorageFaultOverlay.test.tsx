// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { StorageFaultOverlay } from './StorageFaultOverlay'

afterEach(cleanup)

describe('StorageFaultOverlay', () => {
  it('explains the affected card and exposes bounded retained output', () => {
    render(<StorageFaultOverlay sessionTitle="Claude 修复登录"
      fault={{ code: 'STORAGE_QUOTA_EXCEEDED', retainedBytes: 3 * 1024 * 1024 }}
      onRetry={vi.fn()} onEnd={vi.fn()} />)

    expect(screen.getByRole('status', {
      name: '终端记录写入异常：Claude 修复登录'
    })).toBeTruthy()
    expect(screen.getByText('终端已暂停：输出记录写入失败')).toBeTruthy()
    expect(screen.getByText('磁盘空间或存储配额不足')).toBeTruthy()
    expect(screen.getByText('已暂存 3 MB 输出，其他会话不受影响')).toBeTruthy()
    expect(screen.getByRole('button', { name: '重试写入' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '结束会话' })).toBeTruthy()
  })

  it('prevents duplicate retry submissions until the durable result returns', async () => {
    let resolve!: () => void
    const retry = vi.fn(() => new Promise<void>((done) => { resolve = done }))
    render(<StorageFaultOverlay sessionTitle="Shell"
      fault={{ code: 'STORAGE_READ_ONLY', retainedBytes: 17 }}
      onRetry={retry} onEnd={vi.fn()} />)

    const button = screen.getByRole('button', { name: '重试写入' })
    fireEvent.click(button)
    fireEvent.click(button)

    expect(retry).toHaveBeenCalledTimes(1)
    expect(button.hasAttribute('disabled')).toBe(true)
    expect(screen.getByText('正在验证存储并补写输出…')).toBeTruthy()

    resolve()
    await waitFor(() => expect(button.hasAttribute('disabled')).toBe(false))
  })

  it('keeps the overlay actionable after a retry error and protects end from double submit', async () => {
    const retry = vi.fn().mockRejectedValue(new Error('磁盘仍不可写'))
    let finish!: () => void
    const end = vi.fn(() => new Promise<void>((done) => { finish = done }))
    render(<StorageFaultOverlay sessionTitle="Claude"
      fault={{ code: 'STORAGE_WRITE_FAILED', retainedBytes: 0 }} onRetry={retry} onEnd={end} />)

    fireEvent.click(screen.getByRole('button', { name: '重试写入' }))
    expect((await screen.findByRole('alert')).textContent).toContain('磁盘仍不可写')

    const endButton = screen.getByRole('button', { name: '结束会话' })
    fireEvent.click(endButton)
    fireEvent.click(endButton)
    expect(end).toHaveBeenCalledTimes(1)
    expect(endButton.hasAttribute('disabled')).toBe(true)
    expect(screen.getByText('正在安全结束会话…')).toBeTruthy()
    finish()
  })
})
