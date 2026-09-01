import assert from 'node:assert/strict'
import { mkdtemp, readFile, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { prepareRuntimeControlAssets } from './prepare-runtime-control-assets.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = join(root, 'apps/runtime/control-assets')

test('copies executable mt wrappers and complete provider guidance', async () => {
  const temporaryRoot = await mkdtemp(join(tmpdir(), 'matou-control-assets-'))
  const destination = join(temporaryRoot, 'control-assets')
  await prepareRuntimeControlAssets({ source, destination })

  const unixWrapper = await readFile(join(destination, 'bin/mt'), 'utf8')
  const windowsWrapper = await readFile(join(destination, 'bin/mt.cmd'), 'utf8')
  const shared = await readFile(join(destination, 'providers/host-control.md'), 'utf8')
  const claudeSkill = await readFile(
    join(destination, 'providers/claude-plugin/skills/mt-terminal/SKILL.md'), 'utf8'
  )
  const targetRules = await readFile(
    join(destination, 'providers/claude-plugin/skills/mt-terminal/references/target-resolution.md'),
    'utf8'
  )
  const commands = await readFile(
    join(destination, 'providers/claude-plugin/skills/mt-terminal/references/commands.md'), 'utf8'
  )
  const codex = await readFile(
    join(destination, 'providers/codex-developer-instructions.md'), 'utf8'
  )
  const manifest = JSON.parse(await readFile(
    join(destination, 'providers/claude-plugin/.claude-plugin/plugin.json'), 'utf8'
  ))

  assert.match(unixWrapper, /ELECTRON_RUN_AS_NODE=1/)
  assert.match(unixWrapper, /MATOU_CONTROL_NODE_EXECUTABLE/)
  assert.match(unixWrapper, /mt-cli\.cjs/)
  assert.match(windowsWrapper, /ELECTRON_RUN_AS_NODE/)
  assert.equal(manifest.name, 'matou-host-control')
  for (const document of [shared, claudeSkill, targetRules, commands, codex]) {
    assert.match(document, /identify/)
    assert.match(document, /list/)
  }
  for (const command of ['read', 'history', 'commands', 'send', 'key']) {
    assert.match(`${shared}\n${claudeSkill}\n${commands}\n${codex}`, new RegExp(`mt ${command}`))
  }
  assert.match(shared, /最多 5 个|2–5 个/)
  assert.match(shared, /超过 5 个/)
  assert.match(shared, /它.*那个|它\/那个/)
  assert.match(shared, /TARGET_NOT_READY/)
  assert.match(targetRules, /当前画布.*当前 DAG level/)
  assert.match(commands, /不切换焦点/)
  if (process.platform !== 'win32') {
    assert.equal((await stat(join(destination, 'bin/mt'))).mode & 0o777, 0o755)
  }
})
