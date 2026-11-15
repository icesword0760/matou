// @vitest-environment jsdom
import { expect, it, vi } from 'vitest'
import type { MatouDesktopApi } from '../shared/desktop-api'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'

const electron = vi.hoisted(() => ({
  exposeInMainWorld: vi.fn(), send: vi.fn(), invoke: vi.fn(), on: vi.fn()
}))
vi.mock('electron', () => ({
  contextBridge: { exposeInMainWorld: electron.exposeInMainWorld },
  ipcRenderer: { send: electron.send, invoke: electron.invoke, on: electron.on },
  webUtils: {}
}))

it('reports egress country through fire-and-forget IPC without invoking a request', async () => {
  await import('./index')
  const api = electron.exposeInMainWorld.mock.calls[0]![1] as MatouDesktopApi
  expect(api.reportEgressCountry!('JP')).toBeUndefined()
  expect(electron.send).toHaveBeenCalledExactlyOnceWith(DESKTOP_CHANNELS.reportEgressCountry, 'JP')
  expect(electron.invoke).not.toHaveBeenCalled()
  api.reportEgressCountry!(undefined)
  expect(electron.send).toHaveBeenLastCalledWith(DESKTOP_CHANNELS.reportEgressCountry, undefined)
})
