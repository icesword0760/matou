import { expect, it } from 'vitest'
import { routerConfig, type RouterKeys } from './config'
import { CODEX_CLIENT_USER_AGENT } from './codex-client'

const keys: RouterKeys = { client: 'a'.repeat(64), management: 'b'.repeat(64) }

it('keeps Antigravity connections alive with a bounded pool', () => {
  const config = routerConfig('/tmp/matou-router', 51234, keys)
  expect(config.antigravity['connection-pool']).toEqual({ enabled: true, 'idle-conn-timeout': '30s', 'max-idle-conns-per-host': 2 })
  // Credits fallback stays opt-out; pooling must not re-enable it.
  expect(config.oauth.providers.antigravity['antigravity-credits']).toBe(false)
  // Claude Code is the client, so Codex inference must not forward its UA.
  expect(config.oauth.providers.codex['header-defaults']['user-agent']).toBe(CODEX_CLIENT_USER_AGENT)
})

it('adds the engine proxy only when one is resolved', () => {
  expect(routerConfig('/tmp/matou-router', 51234, keys)).not.toHaveProperty('requests')
  expect(routerConfig('/tmp/matou-router', 51234, keys, 'socks5://127.0.0.1:1080')).toMatchObject({ requests: { 'proxy-url': 'socks5://127.0.0.1:1080' } })
})
