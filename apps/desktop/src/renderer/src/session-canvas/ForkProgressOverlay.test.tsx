// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import { activeForkProgress, ForkProgressOverlay } from './ForkProgressOverlay'

afterEach(cleanup)

const progress = {
  operationId: 'operation-1', sessionId: 'session-1', submissionKey: 'submission-1',
  stage: 'applying-setup' as const, completedSteps: 2, totalSteps: 5, attempt: 0
}

describe('ForkProgressOverlay', () => {
  it('covers the card with authoritative stage progress and no cancel action', () => {
    render(<ForkProgressOverlay progress={progress} />)

    expect(screen.getByRole('status', { name: '正在创建分支：正在准备分支环境' })).toBeTruthy()
    expect(screen.getByText('正在执行此工作空间需要的准备步骤')).toBeTruthy()
    expect(screen.getByRole('progressbar').getAttribute('aria-valuenow')).toBe('2')
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('only returns non-terminal progress for the blocking card state', () => {
    expect(activeForkProgress(progress)).toEqual(progress)
    expect(activeForkProgress({ ...progress, stage: 'succeeded' })).toBeUndefined()
    expect(activeForkProgress({ ...progress, stage: 'failed' })).toBeUndefined()
  })
})
