import { existsSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'

import { afterEach, describe, expect, it } from 'vitest'

import { quoteDroppedPath } from './shell-path-quote'

const sideEffectPath = join(tmpdir(), `matou-drop-side-effect-${process.pid}`)

afterEach(() => rmSync(sideEffectPath, { force: true }))

describe('quoteDroppedPath', () => {
  it('keeps reference-visible quoting for ordinary and space-only paths', () => {
    expect(quoteDroppedPath('/tmp/a.txt')).toBe('/tmp/a.txt')
    expect(quoteDroppedPath('/tmp/a b.txt')).toBe('"/tmp/a b.txt"')
  })

  it('quotes leading equals so zsh preserves the original relative path', () => {
    const path = '=ls'
    const quoted = quoteDroppedPath(path)
    const command = `python3 -c 'import json,sys; print(json.dumps(sys.argv[-1]))' -- ${quoted}`
    const result = spawnSync('/bin/zsh', ['-fc', command], { encoding: 'utf8' })

    expect(quoted).toBe("'=ls'")
    expect(result.status, result.stderr).toBe(0)
    expect(JSON.parse(result.stdout.trim())).toBe(path)
  })

  it('rejects NUL because no shell argv or native file path can preserve it', () => {
    expect(quoteDroppedPath('/tmp/a\0b.txt')).toBe('')
  })

  it('uses POSIX single-quote escaping for shell-sensitive and Unicode paths', () => {
    expect(quoteDroppedPath('/tmp/a$(touch PWN).txt')).toBe("'/tmp/a$(touch PWN).txt'")
    expect(quoteDroppedPath("/tmp/a'b.txt")).toBe("'/tmp/a'\\''b.txt'")
    expect(quoteDroppedPath('/tmp/a"b.txt')).toBe("'/tmp/a\"b.txt'")
    expect(quoteDroppedPath('/tmp/a\\b.txt')).toBe("'/tmp/a\\b.txt'")
    expect(quoteDroppedPath('/tmp/a`id`.txt')).toBe("'/tmp/a`id`.txt'")
    expect(quoteDroppedPath('/tmp/a\nb.txt')).toBe("'/tmp/a\nb.txt'")
    expect(quoteDroppedPath('/tmp/你好🚀.txt')).toBe("'/tmp/你好🚀.txt'")
  })

  it('round-trips every sensitive path as one exact argv in real zsh without side effects', () => {
    const samples = [
      '/tmp/a$(touch PWN).txt',
      `/tmp/a$(touch ${sideEffectPath}).txt`,
      "/tmp/a'b.txt",
      '/tmp/a"b.txt',
      '/tmp/a\\b.txt',
      '/tmp/a`printf injected`.txt',
      '/tmp/a\nb.txt',
      '/tmp/a\rb.txt',
      '/tmp/a;printf injected.txt',
      '/tmp/a|cat.txt',
      '/tmp/a>[x]?.txt',
      '/tmp/你好🚀.txt'
    ]

    for (const path of samples) {
      const command = `python3 -c 'import json,sys; print(json.dumps(sys.argv[-1]))' -- ${quoteDroppedPath(path)}`
      const result = spawnSync('/bin/zsh', ['-fc', command], { encoding: 'utf8' })
      expect(result.status, result.stderr).toBe(0)
      expect(JSON.parse(result.stdout.trim())).toBe(path)
    }
    expect(existsSync(sideEffectPath)).toBe(false)
  })
})
