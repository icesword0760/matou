import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { LocaleStore } from './locale-store'

describe('LocaleStore', () => {
  const roots: string[] = []
  const root = () => { const dir = mkdtempSync(join(tmpdir(), 'matou-locale-')); roots.push(dir); return dir }
  afterEach(() => { for (const dir of roots.splice(0)) rmSync(dir, { recursive: true, force: true }) })

  it('follows the system locale when nothing is stored', () => {
    const store = new LocaleStore({ file: join(root(), 'locale.json'), system: () => 'en-US' })
    expect(store.preference()).toBe('system')
    expect(store.current()).toBe('en')
  })

  it('persists an explicit preference and notifies listeners', () => {
    const file = join(root(), 'locale.json')
    const store = new LocaleStore({ file, system: () => 'en-US' })
    const listener = vi.fn()
    store.onChange(listener)
    expect(store.set('zh-CN')).toBe('zh-CN')
    expect(listener).toHaveBeenCalledWith('zh-CN')
    expect(JSON.parse(readFileSync(file, 'utf8'))).toEqual({ preference: 'zh-CN' })
    expect(new LocaleStore({ file, system: () => 'en-US' }).current()).toBe('zh-CN')
  })

  it('lets the environment win and ignores corrupt files', () => {
    const file = join(root(), 'locale.json')
    const store = new LocaleStore({ file, system: () => 'zh-CN', env: 'en' })
    expect(store.current()).toBe('en')
    const broken = new LocaleStore({ file, system: () => 'zh-CN' })
    writeFileSync(file, '{not json')
    expect(broken.preference()).toBe('system')
  })
})
