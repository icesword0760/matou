import { describe, expect, it } from 'vitest'

import { sceneCloseFlow, sessionDeleteFlow, taskDeleteFlow } from './terminal-close-flow'

describe('destructive hierarchy copy', () => {
  it('matches Kooky Task deletion and protects the final Session', () => {
    expect(taskDeleteFlow({ sessionCount: 1, taskName: '修复登录' }).steps.map(({ title }) => title)).toEqual([
      '提示'
    ])
    expect(sessionDeleteFlow({ isWorkspaceFinal: true, taskName: '修复登录' })).toEqual({
      action: 'hide-window', steps: []
    })
  })

  it('uses the approved last-Scene confirmation and no dialog for protected hide', () => {
    expect(sceneCloseFlow({ isLastScene: true, isLastTask: true, taskName: '事项' })).toEqual({
      action: 'hide-window', steps: []
    })
    expect(sceneCloseFlow({ isLastScene: true, isLastTask: false, taskName: '事项' }).steps[0]?.body)
      .toBe('关闭此标签会丢失“事项”中的所有终端会话，但不会删除本地目录。是否继续？')
  })
})
