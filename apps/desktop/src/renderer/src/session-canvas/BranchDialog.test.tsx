// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { BranchDialog } from './BranchDialog'

afterEach(cleanup)

describe('BranchDialog', () => {
  it('opens on the name field with current worktree selected', () => {
    render(<BranchDialog relationMode="child" sourceTitle="Claude"
      gitAvailable onCancel={() => undefined} onConfirm={async () => undefined} />)

    expect(screen.getByRole('dialog', { name: '创建子会话分支' })).toBeTruthy()
    expect(screen.getByRole('textbox', { name: '分支名称' })).toBe(document.activeElement)
    expect((screen.getByRole('radio', { name: /使用当前工作树/ }) as HTMLInputElement).checked).toBe(true)
  })

  it('explains that current-session Fork stays in the current list', () => {
    render(<BranchDialog relationMode="peer" sourceTitle="当前会话"
      gitAvailable onCancel={() => undefined} onConfirm={async () => undefined} />)

    expect(screen.getByRole('dialog', { name: 'Fork 会话' })).toBeTruthy()
    expect(screen.getByText('复制“当前会话”的当前对话并加入当前列表')).toBeTruthy()
    expect(screen.getByText('和当前会话使用同一目录，适合连续处理同一份改动')).toBeTruthy()
  })

  it('submits the trimmed display name and selected new worktree mode', async () => {
    const onConfirm = vi.fn(async () => undefined)
    render(<BranchDialog relationMode="child" sourceTitle="Claude"
      gitAvailable onCancel={() => undefined} onConfirm={onConfirm} />)

    fireEvent.change(screen.getByRole('textbox', { name: '分支名称' }), {
      target: { value: '  修复登录  ' }
    })
    fireEvent.click(screen.getByRole('radio', { name: /从新工作树创建/ }))
    expect(screen.getByText(/原目录中的未提交修改会保留在原处/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: '创建分支' }))

    await waitFor(() => expect(onConfirm).toHaveBeenCalledWith({
      name: '修复登录', worktreeMode: 'new', submissionKey: expect.any(String)
    }))
  })

  it('keeps the new-worktree choice disabled outside a Git repository', () => {
    render(<BranchDialog relationMode="child" sourceTitle="Claude"
      gitAvailable={false} onCancel={() => undefined} onConfirm={async () => undefined} />)

    expect((screen.getByRole('radio', { name: /从新工作树创建/ }) as HTMLInputElement).disabled).toBe(true)
    expect(screen.getByText('需要 Git 仓库')).toBeTruthy()
  })

  it('preserves invalid input and reports inline validation', () => {
    render(<BranchDialog relationMode="sibling" sourceTitle="父会话"
      gitAvailable onCancel={() => undefined} onConfirm={async () => undefined} />)
    const input = screen.getByRole('textbox', { name: '分支名称' })
    fireEvent.change(input, { target: { value: ' '.repeat(3) } })
    fireEvent.click(screen.getByRole('button', { name: '创建分支' }))

    expect(screen.getByText('请输入分支名称')).toBeTruthy()
    expect((input as HTMLInputElement).value).toBe('   ')
    expect(screen.getByRole('dialog', { name: '创建同级分支' })).toBeTruthy()
  })

  it('shows a real workflow error and keeps the user input for correction or retry', async () => {
    const onConfirm = vi.fn(async () => { throw new Error('同一层已存在“修复登录”') })
    render(<BranchDialog relationMode="child" sourceTitle="Claude"
      gitAvailable onCancel={() => undefined} onConfirm={onConfirm} />)
    const input = screen.getByRole('textbox', { name: '分支名称' })
    fireEvent.change(input, { target: { value: '修复登录' } })
    fireEvent.click(screen.getByRole('button', { name: '创建分支' }))

    expect(await screen.findByText('同一层已存在“修复登录”')).toBeTruthy()
    expect((input as HTMLInputElement).value).toBe('修复登录')
    expect((screen.getByRole('button', { name: '创建分支' }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('reuses one submission key when a timed-out intent is confirmed again', async () => {
    const onConfirm = vi.fn()
      .mockRejectedValueOnce(new Error('请求超时'))
      .mockResolvedValueOnce(undefined)
    render(<BranchDialog relationMode="child" sourceTitle="Claude"
      gitAvailable onCancel={() => undefined} onConfirm={onConfirm} />)
    fireEvent.change(screen.getByRole('textbox', { name: '分支名称' }), {
      target: { value: '稳定重试' }
    })

    fireEvent.click(screen.getByRole('button', { name: '创建分支' }))
    expect(await screen.findByText('请求超时')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: '创建分支' }))

    await waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(2))
    const first = onConfirm.mock.calls[0]![0]
    const second = onConfirm.mock.calls[1]![0]
    expect(first.submissionKey).toBe(second.submissionKey)
    expect(first.submissionKey).toEqual(expect.any(String))
  })

  it('coalesces synchronous click and Enter into one authoritative submission', async () => {
    let settle!: () => void
    const pending = new Promise<void>((resolve) => { settle = resolve })
    const onConfirm = vi.fn(() => pending)
    render(<BranchDialog relationMode="sibling" sourceTitle="Claude"
      gitAvailable onCancel={() => undefined} onConfirm={onConfirm} />)
    const input = screen.getByRole('textbox', { name: '分支名称' })
    fireEvent.change(input, { target: { value: '单次提交' } })

    fireEvent.click(screen.getByRole('button', { name: '创建分支' }))
    fireEvent.keyDown(input, { key: 'Enter' })

    expect(onConfirm).toHaveBeenCalledTimes(1)
    settle()
    await pending
  })
})
