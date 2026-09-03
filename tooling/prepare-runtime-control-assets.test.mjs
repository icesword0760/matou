import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, readFile, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

import { prepareRuntimeControlAssets } from './prepare-runtime-control-assets.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = join(root, 'apps/runtime/control-assets')
const execFileAsync = promisify(execFile)

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
  const claudeHooks = JSON.parse(await readFile(
    join(destination, 'providers/claude-plugin/hooks/hooks.json'), 'utf8'
  ))
  const claudeSessionStartPath = join(
    destination, 'providers/claude-plugin/hooks/session-start.mjs'
  )
  const claudeSessionStart = JSON.parse(
    (await execFileAsync(process.execPath, [claudeSessionStartPath])).stdout
  ).hookSpecificOutput.additionalContext

  assert.equal(
    claudeSkill,
    await readFile(join(source, 'providers/claude-plugin/skills/mt-terminal/SKILL.md'), 'utf8')
  )
  assert.equal(
    targetRules,
    await readFile(
      join(source, 'providers/claude-plugin/skills/mt-terminal/references/target-resolution.md'),
      'utf8'
    )
  )
  assert.equal(
    commands,
    await readFile(
      join(source, 'providers/claude-plugin/skills/mt-terminal/references/commands.md'),
      'utf8'
    )
  )
  assert.equal(
    codex,
    await readFile(join(source, 'providers/codex-developer-instructions.md'), 'utf8')
  )

  assert.match(unixWrapper, /ELECTRON_RUN_AS_NODE=1/)
  assert.match(unixWrapper, /MATOU_CONTROL_NODE_EXECUTABLE/)
  assert.match(unixWrapper, /mt-cli\.cjs/)
  assert.match(windowsWrapper, /ELECTRON_RUN_AS_NODE/)
  assert.equal(manifest.name, 'matou-host-control')
  assert.equal(claudeHooks.hooks.SessionStart[0].matcher, 'startup|clear|compact')
  assert.match(
    claudeHooks.hooks.SessionStart[0].hooks[0].command,
    /session-start\.mjs/
  )
  assert.match(claudeSessionStart, /Matou host control is active/)
  assert.match(claudeSessionStart, /invoke `matou-host-control:mt-terminal` immediately/)
  assert.match(claudeSessionStart, /Skip alternate terminal host environment discovery/)
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
  assert.match(claudeSkill, /mt fork children/)
  assert.match(claudeSkill, /创建并分别实现/)
  assert.match(codex, /mt remove preview/)
  assert.match(codex, /只重试失败项/)
  assert.doesNotMatch(codex, /MATOU_CONTROL_TOKEN/)

  for (const providerRules of [claudeSkill, codex]) {
    assert.match(
      providerRules,
      /`mt identify --json` → `mt list --json`\/resolve → 总结批量节点标题 → 仅合并询问缺失环境 → 使用稳定 key 与 `--json` 执行 → 用标题和路径汇报/
    )
    assert.match(
      providerRules,
      /所有分支与 Worktree 环境解析统一使用 `mt list --all --json`；按 `executionContextRef`、`worktreeRef` 去重/
    )
    assert.match(
      providerRules,
      /父节点的 `current` 环境已承载 `main` 时使用 `current`；否则使用解析出的 `existing-worktree`；只有多个真实候选时才集中询问一次/
    )
    assert.match(
      providerRules,
      /仅当至少一项环境缺失时，才把所有缺失项合并成一次询问；用户已逐项给出环境或说“全部 main”时直接解析并提交，不重复确认/
    )
    assert.match(
      providerRules,
      /部分成功.*只重试失败项.*原 batch key.*原 item key/
    )
    assert.match(providerRules, /2–5 个.*CLI.*详情.*选择[\s\S]*超过 5 个.*筛选条件/)
    assert.match(
      providerRules,
      /`mt remove preview`.*`mt close canvas-preview`.*预览.*明确确认.*commit/
    )
    assert.match(providerRules, /项目文件、Git 分支和 Worktree 保持不变/)
    assert.match(
      providerRules,
      /用户输出.*隐藏.*confirmation ref.*内部 ref.*控制凭据/
    )
    assert.doesNotMatch(providerRules, /MATOU_CONTROL_TOKEN/)
  }

  const threeOptionCommand = `cat <<'JSON' | mt fork children self --items-json - --batch-key three-options-v1 --json
[
  {"itemKey":"light","title":"轻量适配方案","environment":{"mode":"current"}},
  {"itemKey":"service","title":"服务层重构方案","environment":{"mode":"new-worktree","branch":"feature/service-refactor"}},
  {"itemKey":"architecture","title":"完整架构升级","environment":{"mode":"existing-worktree","branch":"main","worktreeRef":"worktree:main"}}
]
JSON`
  assert.ok(commands.includes(threeOptionCommand))
  assert.match(commands, /mt list --all --json.*executionContextRef.*worktreeRef.*去重/)
  assert.match(commands, /仅在至少一个方案缺环境时.*全部缺失项.*逐项给出环境.*全部 main.*直接解析并提交/)
  assert.match(commands, /只重试失败项.*原 batch key.*原 item key/)
  assert.match(
    commands,
    /mt remove preview.*--json[\s\S]*mt close canvas-preview.*--json[\s\S]*明确确认[\s\S]*commit/
  )
  assert.match(commands, /项目文件、Git 分支和 Worktree 保持不变/)
  assert.match(commands, /不向用户展示.*确认引用.*内部引用.*控制凭据/)

  assert.match(
    targetRules,
    /所有分支与 Worktree 环境解析统一使用 `mt list --all --json`；按 `executionContextRef`、`worktreeRef` 去重/
  )
  assert.match(
    targetRules,
    /父节点的 `current` 环境已承载 `main` 时使用 `current`；否则使用解析出的 `existing-worktree`；只有多个真实候选时才集中询问一次/
  )
  assert.match(
    targetRules,
    /仅当至少一项环境缺失时，才把所有缺失项合并成一次询问；用户已逐项给出环境或说“全部 main”时直接解析并提交，不重复确认/
  )
  for (const environmentMode of ['current', 'existing-worktree', 'new-worktree']) {
    assert.match(targetRules, new RegExp(`"mode":"${environmentMode}"`))
  }
  assert.match(targetRules, /2–5 个.*CLI.*详情.*选择[\s\S]*超过 5 个.*筛选条件/)
  if (process.platform !== 'win32') {
    assert.equal((await stat(join(destination, 'bin/mt'))).mode & 0o777, 0o755)
  }
})
