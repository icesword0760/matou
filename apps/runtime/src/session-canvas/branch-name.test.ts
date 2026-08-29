import { describe, expect, it } from 'vitest'

import { createGitBranchName, validateDisplayName } from './branch-name'

describe('Fork branch names', () => {
  it('trims a Unicode display name while preserving the user-facing text', () => {
    expect(validateDisplayName('  修复登录 🚀  ', [])).toEqual({
      ok: true,
      displayName: '修复登录 🚀'
    })
  })

  it('reports empty and longer-than-64-codepoint names without replacing the input', () => {
    expect(validateDisplayName('   ', [])).toEqual({
      ok: false,
      code: 'EMPTY',
      message: '请输入分支名称',
      input: '   '
    })
    const tooLong = '会'.repeat(65)
    expect(validateDisplayName(tooLong, [])).toEqual({
      ok: false,
      code: 'TOO_LONG',
      message: '分支名称最多 64 个字符',
      input: tooLong
    })
  })

  it('requires a case-sensitive unique name among active siblings', () => {
    expect(validateDisplayName('修复登录', ['修复登录', '其他分支'])).toEqual({
      ok: false,
      code: 'DUPLICATE',
      message: '同一层已存在“修复登录”',
      input: '修复登录'
    })
    expect(validateDisplayName('Fix Login', ['fix login'])).toEqual({
      ok: true,
      displayName: 'Fix Login'
    })
  })

  it('builds a Git-safe branch from punctuation-heavy Unicode text', () => {
    expect(createGitBranchName('  修复：登录 / API..v2  ', '8B1E04FD-1234-5678-ABCD-1234567890AB'))
      .toBe('matou/修复-登录-api-v2-8b1e04fd')
  })

  it('uses the Session identity to avoid collisions for the same display name', () => {
    const first = createGitBranchName('Feature!', 'session-00000001')
    const second = createGitBranchName('Feature!', 'session-00000002')

    expect(first).toMatch(/^matou\/feature-[a-f0-9]{8}$/)
    expect(second).toMatch(/^matou\/feature-[a-f0-9]{8}$/)
    expect(first).not.toBe(second)
    expect(createGitBranchName('Feature!', 'session-00000001')).toBe(first)
  })
})
