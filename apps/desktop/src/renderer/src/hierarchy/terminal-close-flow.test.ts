import { describe, expect, it } from 'vitest'

import { sceneCloseFlow, taskDeleteFlow } from './terminal-close-flow'

describe('destructive hierarchy copy', () => {
  it('matches reference product Task deletion', () => {
    expect(taskDeleteFlow({ sessionCount: 1, taskName: '修复登录' }).steps.map(({ title }) => title)).toEqual([
      '提示'
    ])
  })

  it('hides the protected final canvas and archives an idle canvas without destructive copy', () => {
    expect(sceneCloseFlow({ isLastScene: true, isLastTask: true, taskName: '事项' })).toEqual({
      action: 'hide-window', steps: []
    })
    expect(sceneCloseFlow({ isLastScene: true, isLastTask: false, taskName: '事项' }))
      .toEqual({ action: 'silent', steps: [] })
  })

  it('shows the affected work count before closing a busy non-final Scene', () => {
    expect(sceneCloseFlow({
      isLastScene: false, isLastTask: true, taskName: '事项',
      runningCount: 2, needsInputCount: 1
    })).toMatchObject({
      action: 'confirm', steps: [{
        title: '关闭画布',
        body: expect.stringContaining('2 个运行中会话和 1 个待输入会话')
      }]
    })
    expect(sceneCloseFlow({
      isLastScene: false, isLastTask: true, taskName: '事项',
      runningCount: 2, needsInputCount: 1
    }).steps[0]?.body).not.toContain('已关闭画布')
  })
})
