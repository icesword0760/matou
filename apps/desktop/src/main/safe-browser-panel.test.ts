import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { net } from 'electron'
import { execFileSync } from 'node:child_process'
import { runningDesktopClients, safeBrowserCheckUpdate } from './safe-browser'

const execute = vi.hoisted(() => vi.fn())
vi.mock('electron', () => ({ app: {}, net: { fetch: vi.fn() } }))
vi.mock('node:child_process', () => ({ spawn: vi.fn(), execFile: vi.fn(), execFileSync: vi.fn() }))
vi.mock('node:util', () => ({ promisify: () => execute }))
vi.mock('node:fs', async original => ({ ...await original<typeof import('node:fs')>(), statSync: () => ({ isFile: () => true }), accessSync: () => {} }))
beforeEach(() => {
  vi.resetAllMocks()
  vi.mocked(execFileSync).mockReturnValue('148.0.7778.215\n')
})
afterEach(() => vi.restoreAllMocks())
describe('safe browser release check', () => {
  it.each([
    ['v148.1.0.0', '148.1.0.0'], ['148.0.7778.216', '148.0.7778.216'],
    ['v148.0.7778.215', null], ['148.0.7778.215.0', null], ['148.0.7778.99', null], ['147.99.9999.999', null]
  ])('compares numeric release %s with installed version', async (tag_name, latest) => {
    vi.mocked(net.fetch).mockResolvedValue(new Response(JSON.stringify({ tag_name })))
    expect(await safeBrowserCheckUpdate()).toEqual({ latest, failed: false })
    expect(net.fetch).toHaveBeenCalledExactlyOnceWith('https://api.github.com/repos/icesword0760/mantle-browser/releases/latest', expect.objectContaining({ signal: expect.any(AbortSignal) }))
  })
  it.each(['http', 'network', 'json', 'tag', 'unknown-installed'])('reports a silent failure for %s', async kind => {
    if (kind === 'http') vi.mocked(net.fetch).mockResolvedValue(new Response('', { status: 404 }))
    if (kind === 'network') vi.mocked(net.fetch).mockRejectedValue(new Error('offline'))
    if (kind === 'json') vi.mocked(net.fetch).mockResolvedValue(new Response('invalid json'))
    if (kind === 'tag') vi.mocked(net.fetch).mockResolvedValue(new Response(JSON.stringify({ tag_name: 'latest' })))
    if (kind === 'unknown-installed') {
      vi.mocked(net.fetch).mockResolvedValue(new Response(JSON.stringify({ tag_name: 'v148.1.0.0' })))
      vi.mocked(execFileSync).mockImplementation(() => { throw new Error('no metadata') })
    }
    expect(await safeBrowserCheckUpdate()).toEqual({ latest: null, failed: true })
  })
})
describe('running desktop clients', () => {
  it('uses macOS pgrep -f patterns scoped to desktop main executables', async () => {
    vi.spyOn(process, 'platform', 'get').mockReturnValue('darwin')
    execute.mockResolvedValue({ stdout: '123\n' })
    expect(await runningDesktopClients()).toEqual(['claude', 'chatgpt'])
    expect(execute).toHaveBeenCalledWith('/usr/bin/pgrep', ['-f', '/Claude\\.app/Contents/MacOS/Claude([[:space:]]|$)'], expect.objectContaining({ timeout: 2000 }))
    expect(execute).toHaveBeenCalledWith('/usr/bin/pgrep', ['-f', '/ChatGPT\\.app/Contents/MacOS/ChatGPT([[:space:]]|$)'], expect.any(Object))
  })
  it('returns only the running client and treats pgrep exit 1 as absent', async () => {
    vi.spyOn(process, 'platform', 'get').mockReturnValue('darwin')
    execute.mockResolvedValueOnce({ stdout: '123\n' }).mockRejectedValueOnce({ code: 1 })
    expect(await runningDesktopClients()).toEqual(['claude'])
    execute.mockRejectedValueOnce({ code: 1 }).mockResolvedValueOnce({ stdout: '456\n' })
    expect(await runningDesktopClients()).toEqual(['chatgpt'])
  })
  it('returns an empty array on a detection error or empty output', async () => {
    vi.spyOn(process, 'platform', 'get').mockReturnValue('darwin')
    execute.mockResolvedValueOnce({ stdout: '123\n' }).mockRejectedValueOnce(new Error('timeout'))
    expect(await runningDesktopClients()).toEqual([])
    execute.mockResolvedValue({ stdout: '' })
    expect(await runningDesktopClients()).toEqual([])
  })
  it('does not execute pgrep on non-macOS systems', async () => {
    vi.spyOn(process, 'platform', 'get').mockReturnValue('win32')
    expect(await runningDesktopClients()).toEqual([])
    expect(execute).not.toHaveBeenCalled()
  })
})
