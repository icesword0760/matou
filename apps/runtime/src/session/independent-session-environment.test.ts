import { describe, expect, it } from 'vitest'
import { CALLER_SESSION_MARKERS, independentSessionEnvironment } from './independent-session-environment'

describe('independent session environment', () => {
  it('removes only caller identity and nesting markers without mutating the parent', () => {
    const retained = {
      HOME: '/fixture/home', PATH: '/fixture/bin', HTTPS_PROXY: 'http://localhost:8080',
      ANTHROPIC_API_KEY: 'fixture-key', CLAUDE_CODE_OAUTH_TOKEN: 'fixture-token',
      ANTHROPIC_BASE_URL: 'http://localhost:8081', CLAUDE_CONFIG_DIR: '/fixture/config',
      CLAUDE_CODE_SKIP_PROMPT_HISTORY: '1', DISABLE_TELEMETRY: '1',
      MATOU_CONTROL_CALLER_SESSION: 'new-card', MATOU_AGENT_STATUS_DIR: '/fixture/new-card'
    }
    const input: NodeJS.ProcessEnv = { ...retained, ...Object.fromEntries(CALLER_SESSION_MARKERS.map(key => [key, 'parent'])) }
    expect(independentSessionEnvironment(input)).toEqual(retained)
    for (const key of CALLER_SESSION_MARKERS) expect(input[key]).toBe('parent')
  })

  it('leaves normal GUI launches unchanged and is idempotent', () => {
    const input = { HOME: '/fixture/home', TERM: 'xterm-256color' }
    expect(independentSessionEnvironment(input)).toEqual(input)
    expect(independentSessionEnvironment(independentSessionEnvironment(input))).toEqual(input)
  })
})
