#!/usr/bin/env node
const assert = require('node:assert/strict')
const { appendFileSync } = require('node:fs')
const { createInterface } = require('node:readline')

const log = (event) => appendFileSync(process.env.CATALOG_FIXTURE_LOG, JSON.stringify(event) + '\n')
process.on('uncaughtException', (error) => {
  log({ error: error.message })
  process.exit(1)
})
const expectedEnv = {
  ANTHROPIC_API_KEY: 'LOCAL_METADATA_ONLY',
  ANTHROPIC_BASE_URL: 'http://127.0.0.1:1',
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1',
  CLAUDE_CONFIG_DIR: process.cwd(),
}
const prefixedEnv = Object.fromEntries(
  Object.entries(process.env).filter(([key]) => /^(ANTHROPIC_|CLAUDE_|MATOU_)/.test(key)),
)
assert.deepEqual(Object.keys(prefixedEnv).sort(), Object.keys(expectedEnv).sort())
for (const [key, value] of Object.entries(expectedEnv)) {
  assert.ok(prefixedEnv[key] === value, 'unexpected value for ' + key)
}
assert.equal(process.env.CATALOG_FIXTURE_SENTINEL, 'preserved')
assert.deepEqual(process.argv.slice(2), [
  '-p', '--input-format', 'stream-json', '--output-format', 'stream-json',
  '--verbose', '--setting-sources', 'user', '--strict-mcp-config',
  '--mcp-config', '{"mcpServers":{}}', '--tools', '',
])
log({ pid: process.pid, cwd: process.cwd(), home: process.env.HOME })

const models = [
  {
    value: 'default', resolvedModel: 'claude-fixture-sonnet', displayName: 'Default fixture',
    supportedEffortLevels: ['low', 'follow', 'high', 'max', 1],
  },
  {
    value: 'sonnet', resolvedModel: 'claude-fixture-sonnet', displayName: 'Sonnet fixture',
    supportedEffortLevels: ['medium', 'xhigh', 'invalid'],
  },
  { value: 'haiku', resolvedModel: 'claude-fixture-haiku' },
  { value: 'opus', resolvedModel: 'claude-fixture-opus' },
  { value: 'fable', resolvedModel: 'claude-fixture-fable', supportedEffortLevels: ['follow'] },
  ...Array.from({ length: 7 }, (_, index) => ({
    value: 'unsupported-' + index, resolvedModel: 'claude-excluded-' + index,
  })),
  { value: 'opus', resolvedModel: 'claude-after-cutoff' },
]

let active
let serial = 0
const mode = process.env.CATALOG_FIXTURE_MODE
if (mode === 'timeout') process.on('SIGTERM', () => {})
const input = createInterface({ input: process.stdin })
input.on('line', (line) => {
  const message = JSON.parse(line)
  log({ message })
  const { type, request_id, request } = message
  assert.equal(type, 'control_request')
  assert.equal(request_id, String(++serial))
  let response
  switch (request.subtype) {
    case 'initialize':
      assert.deepEqual(request, { subtype: 'initialize' })
      response = { models }
      break
    case 'set_model':
      assert.deepEqual(request, { subtype: 'set_model', model: request.model })
      if (request.model === 'sonnet') {
        if (mode === 'timeout') return
        if (mode === 'exit') process.exit(4)
      }
      active = request.model
      if (active === 'fable[1m]') response = false
      else if (active !== 'haiku') response = {}
      break
    case 'get_context_usage':
      assert.deepEqual(request, { subtype: 'get_context_usage', detail: 'summary' })
      response = {
        default: { model: 'claude-fixture-sonnet', rawMaxTokens: 200000 },
        'default[1m]': { model: 'claude-wrong-expanded', rawMaxTokens: 900000 },
        sonnet: { model: 'claude-fixture-sonnet', rawMaxTokens: 210000 },
        'sonnet[1m]': { model: 'claude-fixture-sonnet[1M]', rawMaxTokens: 1000000 },
        opus: { model: 'claude-stale', rawMaxTokens: 200000 },
        fable: { model: 'claude-fixture-fable', rawMaxTokens: 1.5 },
      }[active]
      assert.ok(response, 'unexpected model for get_context_usage')
      break
    default:
      assert.fail('unexpected request subtype: ' + request.subtype)
  }
  process.stdout.write('a non-protocol line\n{"type":"system"}\n')
  process.stdout.write(JSON.stringify({
    type: 'control_response',
    response: { subtype: 'error', request_id: 'unmatched' },
  }) + '\n')
  process.stdout.write(JSON.stringify({
    type: 'control_response',
    response: { subtype: response === undefined ? 'error' : 'success', request_id, response },
  }) + '\n')
})
