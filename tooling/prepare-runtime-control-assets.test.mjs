import assert from 'node:assert/strict'
import { execFile, spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

import { prepareRuntimeControlAssets } from './prepare-runtime-control-assets.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = join(root, 'apps/runtime/control-assets')
const execFileAsync = promisify(execFile)

test('Claude SessionStart appends the managed mt ctx guidance according to the current settings', async (t) => {
  const temporaryRoot = await mkdtemp(join(tmpdir(), 'matou-context-tools-'))
  t.after(() => rm(temporaryRoot, { recursive: true, force: true }))
  const config = join(temporaryRoot, 'context-optimizer.json')
  const hookPath = join(source, 'providers/claude-plugin/hooks/session-start.mjs')
  const env = { ...process.env, MATOU_FOREMAN_DEPTH: '1', MATOU_DATA_ROOT: temporaryRoot }
  const runHook = async (hookEnv = env) => {
    const { stdout } = await execFileAsync(process.execPath, [hookPath], { env: hookEnv })
    return JSON.parse(stdout).hookSpecificOutput.additionalContext
  }
  const rust = await readFile(join(root, 'apps/native/runtime/src/context_guard/mod.rs'), 'utf8')
  const note = rust.match(/const MT_CTX_MANAGED_INSTRUCTIONS: &str = "\\\n([^"]+)";/)?.[1]
  assert.match(note, /mt ctx outline/)
  const baseline = await runHook({ ...env, MATOU_DATA_ROOT: '' })
  for (const settings of [undefined, '{}', '{"agents":{"claude-code":{"enabled":true,"cliTools":true}}}']) {
    if (settings !== undefined) await writeFile(config, settings)
    assert.equal(await runHook(), `${baseline}\n\n${note}`)
  }
  for (const settings of [
    '{"version":1,"agents":{"claude-code":{"enabled":false,"cliTools":true}}}',
    '{"version":1,"agents":{"claude-code":{"cliTools":false}}}',
    '{"version":1,"agents":[]}',
    '{incomplete'
  ]) {
    await writeFile(config, settings)
    assert.equal(await runHook(), baseline)
  }
})

test('copies executable mt wrappers and complete provider guidance', async (t) => {
  const temporaryRoot = await mkdtemp(join(tmpdir(), 'matou-control-assets-'))
  t.after(() => rm(temporaryRoot, { recursive: true, force: true }))
  const destination = join(temporaryRoot, 'control-assets')
  await prepareRuntimeControlAssets({ source, destination })

  const unixWrapper = await readFile(join(destination, 'bin/mt'), 'utf8')
  const windowsWrapper = await readFile(join(destination, 'bin/mt.cmd'), 'utf8')
  const shared = await readFile(join(destination, 'providers/host-control.md'), 'utf8')
  const claudeSkill = await readFile(
    join(destination, 'providers/claude-plugin/skills/mt-terminal/SKILL.md'), 'utf8'
  )
  const foremanSkill = await readFile(
    join(destination, 'providers/claude-plugin/skills/agent-foreman/SKILL.md'), 'utf8'
  )
  const foremanScript = await readFile(
    join(destination, 'providers/claude-plugin/skills/agent-foreman/scripts/foreman.mjs'), 'utf8'
  )
  for (const rel of ['brief-template.md', 'parallel.md', 'reference.md', 'scripts/foreman-statusline.mjs']) {
    const copied = await readFile(join(destination, 'providers/claude-plugin/skills/agent-foreman', rel), 'utf8')
    const original = await readFile(join(source, 'providers/claude-plugin/skills/agent-foreman', rel), 'utf8')
    assert.equal(copied, original)
  }
  assert.equal(
    foremanSkill,
    await readFile(join(source, 'providers/claude-plugin/skills/agent-foreman/SKILL.md'), 'utf8')
  )
  assert.match(foremanSkill, /name:\s*agent-foreman/)
  assert.match(foremanScript, /config\.json/)
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
    (await execFileAsync(process.execPath, [claudeSessionStartPath], {
      env: { ...process.env, MATOU_FOREMAN_ROLES_FILE: '', MATOU_FOREMAN_DEPTH: '' }
    })).stdout
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
  assert.equal(claudeHooks.hooks.SessionStart[0].matcher, 'startup|resume|clear|compact')
  assert.match(
    claudeHooks.hooks.SessionStart[0].hooks[0].command,
    /session-start\.mjs/
  )
  assert.match(claudeSessionStart, /Matou host control is active/)
  assert.match(claudeSessionStart, /invoke `matou-host-control:mt-terminal` immediately/)
  assert.match(claudeSessionStart, /Skip alternate terminal host environment discovery/)
  assert.match(claudeSessionStart, /用户自定义 Agent 生态位与默认配置/)
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

test('Claude SessionStart injects the same dispatch roles as Foreman across lifecycle sources and unreadable settings', async (t) => {
  const temporaryRoot = await mkdtemp(join(tmpdir(), 'matou-session-start-'))
  t.after(() => rm(temporaryRoot, { recursive: true, force: true }))
  const plugin = join(source, 'providers/claude-plugin')
  const hookPath = join(plugin, 'hooks/session-start.mjs')
  const foremanPath = join(plugin, 'skills/agent-foreman/scripts/foreman.mjs')
  const rolesPath = join(temporaryRoot, 'dispatch-roles.json')
  const env = {
    ...process.env, MATOU_DATA_ROOT: '', MATOU_FOREMAN_DEPTH: '', FOREMAN_RUN_DIR: join(temporaryRoot, 'runs'),
    MATOU_FOREMAN_ROLES_FILE: rolesPath
  }
  const readHook = (source, hookEnv = env) => {
    const processResult = spawnSync(process.execPath, [hookPath], {
      env: hookEnv, encoding: 'utf8', timeout: 5000, input: JSON.stringify({ hook_event_name: 'SessionStart', source })
    })
    assert.equal(processResult.error, undefined)
    assert.equal(processResult.status, 0, processResult.stderr)
    const output = JSON.parse(processResult.stdout).hookSpecificOutput
    assert.equal(output.hookEventName, 'SessionStart')
    assert.match(output.additionalContext, /Matou host control is active/)
    return output.additionalContext
  }
  const roleHeading = '## 用户自定义 Agent 生态位与默认配置（以本节为准）'
  const verifyParity = async context => {
    const roles = (await execFileAsync(process.execPath, [foremanPath, 'roles'], { env })).stdout.trimEnd()
    assert.equal(context.slice(context.indexOf(roleHeading)), roles)
  }
  const agents = [
    { id: 'codex', mode: 'auto', model: 'sol', effort: 'low', speed: 'ultrafast', role: ' 第一项\r\n第二项\r第三项 ' },
    { id: 'dsh', enabled: false, role: '不可派' },
    { id: 'agy', mode: 'explicit', model: 'gemini-pro-orchestrator-neon', effort: 'high', role: '咨询' }
  ]
  await writeFile(rolesPath, JSON.stringify({ version: 1, defaultAgent: 'agy', allowSubDispatch: false, agents }))
  for (const source of ['startup', 'resume', 'clear', 'compact']) {
    const context = readHook(source)
    await verifyParity(context)
    assert.match(context, /运行 `mt-foreman roles`/)
    assert.match(context, /速度: 超快.*：第一项\n  第二项\n  第三项/)
    assert.match(context, /默认兜底主力 \| 模型: gemini-pro-orchestrator-neon · 无推理档位/)
    assert.match(context, /已停用（禁止派活）.*DSH/)
    assert.match(context, /已关闭.*Orchestrator 模型内部自带的并行子代理不受影响/)
  }
  assert.doesNotMatch(readHook('startup', { ...env, MATOU_FOREMAN_DEPTH: '1' }), /用户自定义 Agent 生态位/)

  for (const [speed, label] of [['priority', '快速'], ['flex', undefined]]) {
    await writeFile(rolesPath, JSON.stringify({
      version: 1, agents: [{ ...agents[0], speed }, ...agents.slice(1)]
    }))
    const context = readHook('resume')
    await verifyParity(context)
    if (label) assert.match(context, /速度: 快速/)
    else assert.doesNotMatch(context, /速度:/)
  }

  await writeFile(join(temporaryRoot, 'config.json'), JSON.stringify({ version: 1, defaults: { agent: 'agy' } }))
  agents[2].model = 'gpt-oss-120b-medium'
  await writeFile(rolesPath, JSON.stringify({ version: 1, agents }))
  const legacy = readHook('resume')
  await verifyParity(legacy)
  assert.match(legacy, /默认兜底主力 \| 模型: gpt-oss-120b-medium · 推理: 中/)
  assert.match(legacy, /自动派」协作者（Codex）职责范围内的子任务时，跨执行者转派 1 层子任务/)

  const builtins = readHook('startup', { ...env, MATOU_FOREMAN_ROLES_FILE: '' })
  const defaultAgentLines = builtins.split('\n').filter(line => /^- \*\*(Codex|DSH|Antigravity（反重力）)\*\*/.test(line))
  assert.deepEqual(defaultAgentLines, [
    '- **Codex**（默认兜底主力 | 模型: sol · 推理: 高）：方案定好后的一切落地：细节设计、编码与测试、重构、源码阅读与调研、图片/媒体生成、数据处理与打包；电脑界面操作（含 Blender）自动派 terra 最新版子代理完成。',
    '- **DSH**（按职责自动派 | 模型: deepseek-flash · 推理: 高）：前端 UI 设计与界面代码实现：页面和组件的视觉设计、布局、样式、交互外观，以及将其落地为前端界面代码。',
    '- **Antigravity（反重力）**（仅点名时派 | 模型: gemini-pro-orchestrator · 无推理档位）：用户点名时接手落地执行：编码、调研与复杂任务执行（Orchestrator 可自派子代理并行推进；额度耗尽或型号不可用时直接停下汇报，不自动换人）。'
  ])
  assert.match(builtins, /自动派」协作者（Codex、DSH）职责范围内的子任务时，跨执行者转派 1 层子任务/)
  await rm(rolesPath)
  assert.equal(readHook('resume'), builtins)
  assert.equal(readHook('resume', { ...env, MATOU_FOREMAN_ROLES_FILE: temporaryRoot }), builtins)
  await writeFile(rolesPath, '{broken json')
  assert.equal(readHook('clear'), builtins)
  await writeFile(rolesPath, JSON.stringify({ version: 2, agents }))
  assert.equal(readHook('clear'), builtins)

  await writeFile(rolesPath, JSON.stringify({ version: 1, agents: [{ id: 'agy', model: { toString: null }, role: [] }] }))
  const malformedAgent = readHook('compact')
  await verifyParity(malformedAgent)
  assert.match(malformedAgent, /按默认职责执行。/)
  assert.match(malformedAgent, /当前无可转派协作者（需至少启用两个执行者且目标协作者处于「按职责自动派」，当前仅由顶层监工直接派活）/)

  const valid = JSON.stringify({ version: 1, agents })
  const exactLimit = valid + ' '.repeat(256 * 1024 - Buffer.byteLength(valid))
  await writeFile(rolesPath, exactLimit)
  assert.equal(readHook('compact'), legacy)
  await writeFile(rolesPath, exactLimit + ' ')
  assert.equal(readHook('compact'), builtins)
})
