import { describe, expect, it } from 'vitest'

import { sceneCloseFlow, sessionDeleteFlow, taskDeleteFlow } from './terminal-close-flow'

describe('destructive hierarchy copy', () => {
  it('matches reference product Task deletion and protects the final Session', () => {
    expect(taskDeleteFlow({ sessionCount: 1, taskName: '修复登录' }).steps.map(({ title }) => title)).toEqual([
      '提示'
    ])
    expect(sessionDeleteFlow({ isWorkspaceFinal: true, taskName: '修复登录' })).toEqual({
      action: 'hide-window', steps: []
    })
  })

  it('hides the protected final canvas and confirms before closing an idle canvas', () => {
    expect(sceneCloseFlow({
      isLastScene: true, isLastTask: true, taskName: '事项', sceneName: '新画布', sessionCount: 1
    })).toEqual({
      action: 'hide-window', steps: []
    })
    expect(sceneCloseFlow({
      isLastScene: true, isLastTask: false, taskName: '事项', sceneName: '排查画布', sessionCount: 2
    })).toMatchObject({
      action: 'confirm', steps: [{
        title: '关闭画布', confirmLabel: '关闭画布',
        body: expect.stringContaining('“排查画布”下的 2 个会话会全部从界面移除')
      }]
    })
  })

  it('requires confirmation when stopping a running parent Session', () => {
    expect(sessionDeleteFlow({
      isWorkspaceFinal: false, taskName: '事项', sessionTitle: '主会话',
      workStatus: 'running', childCount: 3
    })).toMatchObject({
      action: 'confirm',
      steps: [{
        title: '停止会话', confirmLabel: '停止会话',
        body: expect.stringContaining('正在运行，并有 3 个子会话')
      }]
    })
  })

  it('lets a newly starting leaf Session be cancelled without a running-work confirmation', () => {
    expect(sessionDeleteFlow({
      isWorkspaceFinal: false, taskName: '事项', sessionTitle: '新 Shell',
      workStatus: 'starting', childCount: 0
    })).toEqual({ action: 'silent', steps: [] })
  })

  it('shows the affected work count before closing a busy non-final Scene', () => {
    expect(sceneCloseFlow({
      isLastScene: false, isLastTask: true, taskName: '事项', sceneName: '排查画布', sessionCount: 4,
      runningCount: 2, needsInputCount: 1
    })).toMatchObject({
      action: 'confirm', steps: [{
        title: '关闭画布',
        body: expect.stringContaining('其中 2 个运行中会话和 1 个待输入会话将停止')
      }]
    })
    expect(sceneCloseFlow({
      isLastScene: false, isLastTask: true, taskName: '事项', sceneName: '排查画布', sessionCount: 4,
      runningCount: 2, needsInputCount: 1
    }).steps[0]?.body).not.toContain('已关闭画布')
  })
})
