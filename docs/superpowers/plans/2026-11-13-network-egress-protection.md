# 网络出口检测与出口保护 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. In this repo, execution is dispatched through the `agent-foreman` skill (Codex), one task per brief, with Claude reviewing code and running the app between tasks.

**Goal:** Matou 持续检测官方 agent 会话真实的网络出口（服务商眼中的 IP 和国家），出口在中国大陆或违反出口规则时冻结对应服务商的会话、恢复后自动继续，并把会话里的大陆时区换成东八区非大陆时区。

**Architecture:** 新增 runtime 单例 `EgressMonitor`（`apps/runtime/src/egress/`）。它用与 agent 会话相同的代理环境变量，请求服务商 Cloudflare 的 `/cdn-cgi/trace`，按服务商（anthropic、openai）分别评估，通过 `RuntimeSessionRegistry` 冻结或解冻 `PtySession`，冻结复用 SIGSTOP 暂停能力，并按暂停原因计数。状态以全局消息 `egress.status` 推给渲染进程。渲染进程在 HUD 显示出口状态，冻结时在卡片顶部显示横幅，另提供检查面板，并在冻结和恢复时发通知。时区复用现有的 `claude.timeZone` 偏好，新增 `auto` 取值。

**Tech Stack:** TypeScript、Node ≥22.16、node-pty、Electron + React（renderer）、vitest + Testing Library、pnpm workspace；新增 runtime 依赖 `https-proxy-agent@7.0.6`、`socks-proxy-agent@8.0.5`。

**Spec:** `docs/prd/PRD-Matou-网络出口检测.md`

## Global Constraints

- 不内置代理、不开 TUN、不改系统设置；只读系统代理配置用于 W2。
- 探测必须走 agent 会话的环境变量路径；**禁止用 Electron `net`/`session` 发探测请求**（它走系统代理）。
- 官方会话判定：`claude-code` 且 `ANTHROPIC_BASE_URL` 缺省或主机为 `api.anthropic.com` → `anthropic`；`codex` 且 `OPENAI_BASE_URL` 缺省或主机为 `api.openai.com` → `openai`；其余（Shell、第三方服务商）一律不探测、不冻结、不改时区。
- 探测地址固定：anthropic → `api.anthropic.com`（API）、`claude.ai`（登录）；openai → `api.openai.com`（API）、`chatgpt.com`（登录）。路径 `/cdn-cgi/trace`，单次超时 5000 ms，不跟随重定向，响应体上限 4 KiB。
- 大陆判定只看 trace 的 `loc=CN`；`HK`/`MO`/`TW` 不算大陆。不内置网段库。
- 时序常量：轻量采样 60 000 ms；网络变化轮询 5 000 ms（定时器漂移 > 15 000 ms 视为唤醒）；冻结期复检 15 000 ms；自动继续前二次确认间隔 5 000 ms；恢复横幅显示 3 000 ms。
- 冻结只在**确认**违规（拿到 `loc=CN`，或 API 出口违反规则）时发生；超时、无法解析 → `unknown`，既不冻结也不解冻。
- 东八区替换：大陆/港澳时区集合为 `Asia/Shanghai Asia/Chongqing Asia/Chungking Asia/Harbin Asia/Urumqi Asia/Kashgar PRC Asia/Hong_Kong Hongkong Asia/Macau Asia/Macao`；替换目标为 `TW→Asia/Taipei`、`MY→Asia/Kuala_Lumpur`、`PH→Asia/Manila`，其余 `Asia/Singapore`。只设 `TZ`，**不碰** `LANG`/`LC_*`。
- 偏好存储复用 `preferences` 表：`claude.timeZone`（新增 `auto`，未设置时默认 `auto`）、`egress.protection`（boolean，默认 true）、`egress.rule`（`{ mode: 'country'|'ip', baselines: { anthropic?, openai? } }`，默认 `country`）。
- 所有用户可见文案按现有 `t(zh, en)` 模式同时提供中英文；文案取自 PRD。
- 依赖写精确版本；提交前运行 `pnpm check:identifiers`；源码不出现外部产品品牌名。
- Windows：冻结只阻止输入、暂停输出读取（现有 pauser 在 win32 不发信号），状态与横幅照常。

## Review Focus

1. 冻结中的会话被关闭、删除或 App 退出：被 SIGSTOP 的进程收不到 SIGHUP/SIGTERM，可能卡住退出。期望先 SIGCONT 再 kill。（Task 6 测试）
2. 写盘背压暂停与出口冻结叠加：背压恢复时不能把出口冻结的进程放开，反之亦然。（Task 6 测试）
3. 节点抖动（通、断、通）：不能反复冻结、解冻，必须连续两次通过才继续。（Task 7 测试）
4. 断网、全部探测超时：已冻结的保持冻结，未冻结的不冻结，状态为「无法确认」。（Task 7 测试）
5. 异常期间新起的会话一出生就冻结；用户对当前出口点了「仍要继续」后，同一出口下新起的会话不再冻结。出口再变化时越过失效。（Task 7 测试）

---

### Task 1: 协议与类型

**Files:**
- Create: `packages/contracts/src/egress.ts`
- Modify: `packages/contracts/src/protocol.ts`（RPC 方法列表、`RuntimeMessage` 联合）、`packages/contracts/src/index.ts`
- Test: `packages/contracts/src/egress.test.ts`

**Interfaces:**
- Produces（后续所有任务使用）：

```ts
export type EgressFamily = 'anthropic' | 'openai'
export type EgressCheckId = 'mainland' | 'exit-rule' | 'login-api-match' | 'agent-proxy-path' | 'browser-path' | 'time-zone' | 'ipv6'
export type EgressCheckResult = 'pass' | 'warn' | 'fail' | 'unknown'
export type EgressHostError = 'timeout' | 'unreachable' | 'invalid'
export interface EgressExit { ip: string; country: string }            // country = ISO 3166-1 alpha-2, upper case
export interface EgressHostSample { host: string; ip?: string; country?: string; error?: EgressHostError }
export interface EgressCheckWire { id: EgressCheckId; family?: EgressFamily; result: EgressCheckResult; facts?: Record<string, string> }
export interface EgressFamilyWire {
  family: EgressFamily
  state: 'ok' | 'warning' | 'abnormal' | 'unknown'
  exit?: EgressExit
  hosts: EgressHostSample[]
  violation?: 'mainland' | 'rule'
  baseline?: EgressExit
  overridden: boolean
  frozenSessionIds: string[]
  recovering: boolean
}
export interface EgressStatusWire {
  state: 'checking' | 'ok' | 'warning' | 'abnormal'
  families: EgressFamilyWire[]
  checks: EgressCheckWire[]
  ruleMode: 'country' | 'ip'
  protectionEnabled: boolean
  sessionFamilies: Record<string, EgressFamily>
  checkedAt?: number
  lastTransition?: { id: string; kind: 'frozen' | 'resumed'; family: EgressFamily; sessionCount: number; at: number; reason?: 'mainland' | 'rule' }
}
export function isEgressFamily(value: unknown): value is EgressFamily
```

- RPC 方法：`'egress.status'`（无参，返回 `EgressStatusWire`）、`'egress.recheck'`、`'egress.set-rule'`（`{ mode }`）、`'egress.accept-current'`（`{ family }`）、`'egress.set-protection'`（`{ enabled }`）、`'egress.override'`（`{ family }`）。后五个都返回最新的 `EgressStatusWire`。
- 消息：`{ type: 'egress.status'; protocolVersion; status: EgressStatusWire }`。

- [ ] **Step 1:** 写 `egress.test.ts`：`isEgressFamily('anthropic') === true`、`isEgressFamily('google') === false`；断言 `RPC_METHODS` 包含上述 6 个方法。
- [ ] **Step 2:** `pnpm --filter @matou/contracts test`，确认失败。
- [ ] **Step 3:** 实现类型、守卫，并把方法和消息加进协议。
- [ ] **Step 4:** 测试通过；运行 `pnpm typecheck`，在消息 `switch` 处补全穷尽检查（只加 `case 'egress.status': break` 占位，不做行为）。
- [ ] **Step 5:** Commit `feat(egress): add egress status protocol types`

### Task 2: 代理路由与 trace 探测

**Files:**
- Create: `apps/runtime/src/egress/proxy-route.ts`、`apps/runtime/src/egress/trace-probe.ts`
- Modify: `apps/runtime/package.json`（加 `"https-proxy-agent": "7.0.6"`、`"socks-proxy-agent": "8.0.5"`）
- Test: `apps/runtime/src/egress/proxy-route.test.ts`、`apps/runtime/src/egress/trace-probe.test.ts`

**Interfaces:**
- Produces:

```ts
export type Environment = Record<string, string | undefined>
export function resolveProxyForUrl(env: Environment, url: string): string | undefined
export function hasProxyEnvironment(env: Environment): boolean
export function parseTrace(body: string): { ip?: string; country?: string }
export type TraceRequester = (url: string, proxyUrl: string | undefined, timeoutMs: number) => Promise<string>
export function probeTrace(host: string, proxyUrl: string | undefined, options?: { timeoutMs?: number; request?: TraceRequester }): Promise<EgressHostSample>
```

- [ ] **Step 1:** 写 `proxy-route.test.ts`，覆盖以下用例：
  - https URL 取 `https_proxy`；小写优先于 `HTTPS_PROXY`；缺失时回落 `all_proxy`/`ALL_PROXY`。
  - http URL 取 `http_proxy`。
  - `NO_PROXY=*` → `undefined`。
  - `NO_PROXY=.anthropic.com` 与 `NO_PROXY=anthropic.com` 都命中 `api.anthropic.com`，但不命中 `notanthropic.com`。
  - 无协议头的值 `127.0.0.1:7890` → `http://127.0.0.1:7890`。
  - `socks5h://127.0.0.1:7891` 原样返回。
  - `hasProxyEnvironment({ ALL_PROXY: 'x' }) === true`、`hasProxyEnvironment({}) === false`。
- [ ] **Step 2:** 写 `trace-probe.test.ts`，覆盖：
  - `parseTrace("fl=1\nh=api.anthropic.com\nip=169.128.1.2\nloc=US\n")` → `{ ip: '169.128.1.2', country: 'US' }`。
  - `loc=usa`、非法 ip → 对应字段 `undefined`。
  - `probeTrace` 用注入的 `request`：返回正文 → 样本带 ip 和国家；抛 `ETIMEDOUT` 或超时 → `error: 'timeout'`；抛 `ECONNREFUSED` → `'unreachable'`；正文缺字段 → `'invalid'`。
  - 断言请求 URL 为 `https://api.anthropic.com/cdn-cgi/trace`，超时为 5000。
- [ ] **Step 3:** 运行 `pnpm --filter @matou/runtime exec vitest run src/egress`，确认失败。
- [ ] **Step 4:** 实现。默认 requester 用 `node:https.get`：`http(s)` 代理用 `HttpsProxyAgent`，`socks*` 用 `SocksProxyAgent`，`agent: false` 表示直连；3xx 视为 `invalid`；正文超过 4096 字节即中止。
- [ ] **Step 5:** 测试通过后 `pnpm install`，确认 `pnpm build:runtime` 能打包新依赖。
- [ ] **Step 6:** Commit `feat(egress): resolve agent proxy route and probe provider trace`

### Task 3: 本机网络事实

**Files:**
- Create: `apps/runtime/src/egress/local-network.ts`
- Test: `apps/runtime/src/egress/local-network.test.ts`

**Interfaces:**
- Produces:

```ts
export type Interfaces = NodeJS.Dict<import('node:os').NetworkInterfaceInfo[]>
export function parseScutilProxy(text: string): string | undefined
export function readSystemProxy(platform?: NodeJS.Platform): Promise<string | undefined>
export function detectTun(interfaces: Interfaces): boolean
export function hasGlobalIpv6(interfaces: Interfaces): boolean
export function canReachIpv6Directly(timeoutMs?: number): Promise<boolean>
export function networkSignature(interfaces: Interfaces): string
```

- [ ] **Step 1:** 写测试，覆盖：
  - `parseScutilProxy`：`HTTPSEnable : 1` 配 `HTTPSProxy`/`HTTPSPort` → `http://127.0.0.1:7890`；仅 SOCKS 启用 → `socks5://…`；仅 HTTP 启用 → `http://…`；全部为 0 → `undefined`。
  - `detectTun`：任一非 internal 接口有 `198.18.0.0/15` 内的 IPv4 → true；只有 `192.168.1.5` → false。
  - `hasGlobalIpv6`：`fe80::1`、`fd00::1`、internal 均为 false；`2408:…` 为 true。
  - `networkSignature`：接口顺序不同但内容相同 → 相同签名；增删地址 → 签名改变。
- [ ] **Step 2:** 确认测试失败。
- [ ] **Step 3:** 实现。`readSystemProxy` 只在 `darwin` 上执行 `scutil --proxy`，超时 3000 ms，失败返回 `undefined`；`canReachIpv6Directly` 用 `net.connect({ host: '2606:4700:4700::1111', port: 443 })`，不走代理，默认超时 3000 ms。
- [ ] **Step 4:** 确认测试通过。
- [ ] **Step 5:** Commit `feat(egress): read local proxy, tun and ipv6 facts`

### Task 4: 评估（纯函数）

**Files:**
- Create: `apps/runtime/src/egress/egress-evaluator.ts`
- Test: `apps/runtime/src/egress/egress-evaluator.test.ts`

**Interfaces:**
- Consumes: Task 1 类型。
- Produces:

```ts
export const EGRESS_HOSTS: Record<EgressFamily, { api: string; login: string }>
export interface EgressSample {
  hosts: Record<EgressFamily, { api: EgressHostSample; login: EgressHostSample }>
  browserLogin?: EgressHostSample           // claude.ai via system proxy (darwin, system proxy enabled)
  agentHasProxyEnv: boolean
  systemProxyEnabled: boolean
  tunDetected: boolean
  ipv6Bypass: boolean
  agentTimeZoneMainland: boolean            // effective TZ of a new official session is mainland/HK/Macau (computed by the sampler via Task 5 policy)
}
export interface FamilyEvaluation { family: EgressFamily; state: 'ok' | 'warning' | 'abnormal' | 'unknown'; exit?: EgressExit; violation?: 'mainland' | 'rule' }
export interface EgressEvaluation { families: FamilyEvaluation[]; checks: EgressCheckWire[]; state: 'ok' | 'warning' | 'abnormal' }
export function evaluateEgress(sample: EgressSample, rule: { mode: 'country' | 'ip'; baselines: Partial<Record<EgressFamily, EgressExit>> }): EgressEvaluation
```

规则（按服务商）：
- **mainland：** api 或 login 任一 `country === 'CN'` → `fail`，`violation: 'mainland'`，`state: 'abnormal'`。两者都没取到 ip 和国家 → `unknown`。
- **exit-rule：** 只看 api 样本。有基准时：`country` 模式比国家，`ip` 模式比 ip，不同 → `fail`，`violation: 'rule'`。无基准或 api 未知 → `pass`/`unknown`。
- **login-api-match：** api 和 login 都取到且 ip 不同 → `warn`，`facts: { apiIp, loginIp }`。
- **exit：** 优先取 api 样本，否则取 login 样本。

全局提醒：
- **agent-proxy-path：** `systemProxyEnabled && !agentHasProxyEnv && !tunDetected` → `warn`。
- **browser-path：** `browserLogin.ip` 与 anthropic login 的 ip 不同 → `warn`。
- **time-zone：** `agentTimeZoneMainland` → `warn`。
- **ipv6：** `ipv6Bypass` → `warn`。

整体状态：任一服务商 `abnormal` → `abnormal`；否则有任何 `warn` 或 `unknown` → `warning`；否则 `ok`。

- [ ] **Step 1:** 写测试：
  - 全 US 且一致 → `ok`。
  - anthropic api 为 CN、openai 为 US → anthropic `abnormal`/`mainland`，openai `ok`，整体 `abnormal`。
  - `claude.ai` 为 US 1.1.1.1、api 为 US 2.2.2.2 → `login-api-match` 为 `warn`，anthropic 状态 `warning`，不 `abnormal`。
  - anthropic 走 US、openai 走 JP → 两家都 `ok`。
  - 基准 US、`country` 模式、api 为 JP → `rule`；同场景改 `ip` 模式、同国家换 ip → `rule`。
  - 全部 `timeout` → 两家 `unknown`，整体 `warning`，无 `violation`。
  - 系统代理开、无环境变量、无 TUN → `agent-proxy-path` 为 `warn`。
  - `agentTimeZoneMainland: true` → `time-zone` 为 `warn`；`false` → `pass`。
- [ ] **Step 2:** 确认失败。
- [ ] **Step 3:** 实现。
- [ ] **Step 4:** 确认通过。
- [ ] **Step 5:** Commit `feat(egress): evaluate exit per provider`

### Task 5: 时区策略与设置

**Files:**
- Create: `apps/runtime/src/egress/time-zone-policy.ts`
- Modify: `apps/runtime/src/session/claude-settings.ts`、`packages/contracts/src/claude-settings.ts`（注释说明 `auto`）、`apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.tsx`
- Test: `apps/runtime/src/egress/time-zone-policy.test.ts`、`apps/runtime/src/session/claude-settings.test.ts`、`apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.test.tsx`

**Interfaces:**
- Produces:

```ts
export function isMainlandTimeZone(zone: string): boolean
export function nonMainlandUtc8Zone(exitCountry?: string): string
export function agentTimeZone(input: { profile: 'claude-code' | 'codex'; setting: string; systemTimeZone: string; exitCountry?: string }): string | undefined
// ClaudeSettings
environment(profile: string, options?: { exitCountry?: string; official?: boolean }): Record<string, string>
```

`agentTimeZone` 规则：
- `codex` 一律按 `auto` 处理。
- `system` → `undefined`。
- 具体 IANA 时区 → 原样使用（仅 claude）。
- `auto` → 系统时区是大陆或港澳时，取 `nonMainlandUtc8Zone(exitCountry)`；否则 `undefined`。

`environment` 在 `official === false` 时，只保留用户显式选的 IANA 时区（与现状一致），`auto` 不替换。

- [ ] **Step 1:** 写策略测试：
  - `nonMainlandUtc8Zone('TW') === 'Asia/Taipei'`、`('US') === 'Asia/Singapore'`、`(undefined) === 'Asia/Singapore'`。
  - `agentTimeZone({ profile:'claude-code', setting:'auto', systemTimeZone:'Asia/Shanghai', exitCountry:'US' }) === 'Asia/Singapore'`。
  - 系统时区为 `Europe/Berlin` 时，`auto` → `undefined`。
  - `setting:'system'` → `undefined`。
  - codex 配 `setting:'America/New_York'` 且系统时区为上海 → `Asia/Singapore`。
- [ ] **Step 2:** 更新 `claude-settings.test.ts`：
  - 未设置时 `snapshot().timeZone === 'auto'`。
  - `set('auto')` 合法。
  - `environment('codex', { exitCountry: 'TW', official: true })` 在系统时区为上海时 → `{ TZ: 'Asia/Taipei' }`（用注入的系统时区或 `TZ` 测试夹具）。
  - `official:false` 且 `auto` → `{}`。
- [ ] **Step 3:** 更新组件测试：第一项为「自动（隐藏大陆时区，推荐）」/「Automatic (hide mainland China time zone, recommended)」，值为 `auto`，默认选中；预览显示替换后的时区。
- [ ] **Step 4:** 确认三处测试失败。
- [ ] **Step 5:** 实现。`isClaudeTimeZone` 接受 `'auto'`；`ClaudeSettings` 构造函数可选注入 `systemTimeZone` 以便测试；组件说明文案改为：「自动：本机是大陆时区时，Claude Code 和 Codex 使用同为东八区的新加坡时区（出口在台湾时用台北），时间读数不变。」。
- [ ] **Step 6:** 确认通过。
- [ ] **Step 7:** Commit `feat(egress): hide mainland time zone for official agent sessions`

### Task 6: 按原因暂停与会话冻结

**Files:**
- Modify: `apps/runtime/src/session/pty-execution-pauser.ts`、`apps/runtime/src/session/pty-session.ts`
- Test: `apps/runtime/src/session/pty-execution-pauser.test.ts`、`apps/runtime/src/session/pty-session.test.ts`

**Interfaces:**
- Produces:

```ts
export type PauseReason = 'durability' | 'egress'
export class SharedExecutionPauser {
  constructor(inner: DurabilityExecutionPauser)
  lease(reason: PauseReason): DurabilityExecutionPauser
  isPausedFor(reason: PauseReason): boolean
}
export class SessionEgressFrozenError extends Error {}
// PtySession
get egressFrozen(): boolean
setEgressFrozen(frozen: boolean): Promise<void>
```

- [ ] **Step 1:** 写 pauser 测试（用计数假 inner）：
  - 两个 lease 依次 pause，inner 只 pause 1 次。
  - durability resume 后 inner 不 resume。
  - egress 也 resume 后，inner resume 1 次。
  - 同一 lease 重复 pause 或 resume 是幂等的。
- [ ] **Step 2:** 写 PtySession 测试（真实 `/bin/sh` 子进程，参照现有 pauser 测试的进程夹具）：
  - `setEgressFrozen(true)` 后 `write('x')` 抛 `SessionEgressFrozenError`，`egressFrozen === true`。
  - 冻结中 `dispose()` 在 3 s 内让进程退出（先 SIGCONT 再 kill）。
  - 冻结中 `shutdownForRuntime()` 正常结束。
  - 冻结后，持久化背压到达低水位而 resume 时，进程仍处于 stopped 状态（读 `ps -o stat= -p <pid>` 含 `T`）。
- [ ] **Step 3:** 确认失败。
- [ ] **Step 4:** 实现。`PtySession` 构造时创建 `SharedExecutionPauser(new PtyExecutionPauser(terminal))`，把 `lease('durability')` 交给 `SessionDurabilityGate`，自己持有 `lease('egress')`；`write()` 先检查冻结；`dispose`、`shutdownForRuntime` 在 kill 前先 `setEgressFrozen(false)`。
- [ ] **Step 5:** 确认通过；运行 `pnpm --filter @matou/runtime exec vitest run src/session`，原有用例全部通过。
- [ ] **Step 6:** Commit `feat(egress): freeze sessions with reason-counted pausing`

### Task 7: 出口设置与监控状态机

**Files:**
- Create: `apps/runtime/src/egress/egress-settings.ts`、`apps/runtime/src/egress/egress-monitor.ts`
- Test: `apps/runtime/src/egress/egress-settings.test.ts`、`apps/runtime/src/egress/egress-monitor.test.ts`

**Interfaces:**
- Consumes: Task 2–6。
- Produces:

```ts
// egress-settings.ts
export class EgressSettings {
  constructor(database: RuntimeDatabase)
  protectionEnabled(): boolean
  setProtection(enabled: boolean): void
  rule(): { mode: 'country' | 'ip'; baselines: Partial<Record<EgressFamily, EgressExit>> }
  setMode(mode: 'country' | 'ip'): void
  setBaseline(family: EgressFamily, exit: EgressExit): void
}
// egress-monitor.ts
export interface EgressMonitorDependencies {
  now(): number
  setTimer(callback: () => void, ms: number): () => void      // returns cancel
  sampleFull(): Promise<EgressSample>
  sampleLight(): Promise<EgressHostSample>                    // anthropic api via agent route
  networkSignature(): string
  settings: EgressSettings
  freeze(sessionId: string): Promise<void>
  thaw(sessionId: string): Promise<void>
  publish(status: EgressStatusWire): void
}
export class EgressMonitor {
  constructor(deps: EgressMonitorDependencies)
  start(): void
  stop(): void
  registerSession(sessionId: string, family: EgressFamily): { bornFrozen: boolean }
  unregisterSession(sessionId: string): void
  exitCountry(family: EgressFamily): string | undefined
  status(): EgressStatusWire
  recheck(): Promise<EgressStatusWire>
  setRuleMode(mode: 'country' | 'ip'): Promise<EgressStatusWire>
  acceptCurrent(family: EgressFamily): Promise<EgressStatusWire>
  setProtection(enabled: boolean): Promise<EgressStatusWire>
  override(family: EgressFamily): Promise<EgressStatusWire>
}
export function egressFamilyForLaunch(profile: string, providerEnv: Record<string, string>): EgressFamily | undefined
```

状态机：
- `start()` 立即跑一次完整检查。之后每 60 000 ms 轻量采样：ip 变化或失败转为成功时触发完整检查。每 5 000 ms 比较网络签名，签名变化或两次轮询间隔超过 15 000 ms 时，触发完整检查。
- 完整检查后，某服务商出现确认的违规，且保护已开启、未被越过 → 冻结该服务商所有已登记会话，记录 `lastTransition{kind:'frozen'}`，并进入复检：每 15 000 ms 一次。
- 复检通过后，5 000 ms 再确认一次。两次都通过才解冻该服务商被冻结的会话，记录 `lastTransition{kind:'resumed'}`；任一次失败或 unknown 就回到 15 000 ms 复检。
- 首次通过 mainland 检查、且该服务商还没有基准时，用 api 样本写入基准。
- `override(family)`：记录 `{ family, ip: exit.ip }`，立即解冻。之后该服务商出口 ip 与越过记录相同时不冻结；ip 变化后越过失效。
- `acceptCurrent(family)`：当前 api 样本必须通过 mainland 检查，满足时写入基准，并立即重新评估；否则抛错。
- `setProtection(false)`：立即解冻全部会话，之后不再冻结。
- `registerSession`：该服务商当前为 `abnormal`、保护开启、未越过 → 返回 `bornFrozen: true`，并计入冻结列表。

- [ ] **Step 1:** 写 `egress-settings.test.ts`（用内存数据库夹具，参照 `claude-settings.test.ts`）：默认 `protectionEnabled() === true`、`rule().mode === 'country'`；写入后重开仍保留；损坏的 JSON 回落默认值。
- [ ] **Step 2:** 写 `egress-monitor.test.ts`（假时钟、假采样、记录 freeze/thaw）：
  - anthropic CN → 只冻结 anthropic 会话，codex 会话不动，`lastTransition.kind === 'frozen'`。
  - 冻结后 15 s 复检通过，5 s 再确认通过 → thaw，`lastTransition.kind === 'resumed'`。
  - 抖动：复检通过，二次确认失败，再通过一次 → 不 thaw，直到连续两次通过。（Review Focus 3）
  - 全部超时：未冻结的不冻结；已冻结的保持冻结。（Review Focus 4）
  - `override` → 立即 thaw；同 ip 新注册的会话 `bornFrozen === false`；出口 ip 变化后再遇违规会重新冻结。（Review Focus 5）
  - 异常时 `registerSession` → `bornFrozen === true`。
  - 网络签名变化 → 5 s 内触发完整检查；时钟跳过 20 s → 触发完整检查。
  - `setProtection(false)` → 解冻全部会话，后续违规不冻结。
  - 首次通过时写入基准；`country` 模式下换国家 → `rule` 冻结；`acceptCurrent` 后解冻。
  - `egressFamilyForLaunch('claude-code', {})` → `'anthropic'`；`ANTHROPIC_BASE_URL: 'https://api.deepseek.com/anthropic'` → `undefined`；`codex` 配 `https://api.openai.com/v1` → `'openai'`；`shell` → `undefined`。
- [ ] **Step 3:** 确认失败。
- [ ] **Step 4:** 实现。
- [ ] **Step 5:** 确认通过。
- [ ] **Step 6:** Commit `feat(egress): monitor exits and freeze or resume provider sessions`

### Task 8: Runtime 接线

**Files:**
- Create: `apps/runtime/src/egress/egress-sampler.ts`（组装 `sampleFull` 和 `sampleLight`）
- Modify: `apps/runtime/src/index.ts`、`apps/runtime/src/runtime-server.ts`（`RuntimeServerOptions.egress`、会话生成处约 L1900–2140、RPC 拦截处约 L1130–1290、会话退出处）
- Test: `apps/runtime/src/egress/egress-sampler.test.ts`、`apps/runtime/src/runtime-server.test.ts`（新增 describe）

**Interfaces:**
- Consumes: `EgressMonitor`、`egressFamilyForLaunch`、`ClaudeSettings.environment(profile, { exitCountry, official })`、`PtySession.setEgressFrozen`。
- Produces:

```ts
export function agentProxyEnvironment(family: EgressFamily, base: Environment, claudeUserSettingsPath: string): Promise<Environment>
export function createEgressSampler(input: { baseEnv: Environment; claudeUserSettingsPath: string; agentTimeZoneMainland(): boolean }): { sampleFull(): Promise<EgressSample>; sampleLight(): Promise<EgressHostSample> }
// RuntimeServerOptions
egress?: EgressMonitor
```

- [ ] **Step 1:** 写 sampler 测试：
  - `~/.claude/settings.json` 的 `env.HTTPS_PROXY` 会合并进 anthropic 路由，openai 路由不合并。
  - settings 文件缺失或损坏时，只用基础环境。
  - `sampleFull` 对 4 个主机并发探测，系统代理开启时额外探测 `claude.ai`。（注入 `probeTrace`、`readSystemProxy`）
- [ ] **Step 2:** 写 runtime-server 测试（用注入的假 monitor）：
  - 官方 `claude-code` 会话生成时调用 `registerSession(id, 'anthropic')`。
  - `bornFrozen: true` 时，`PtySession` 生成后立即 `setEgressFrozen(true)`。
  - 第三方服务商会话不登记。
  - 会话退出时 `unregisterSession`。
  - `egress.*` 六个 RPC 转发到 monitor 并返回状态。
  - `publish` 向所有已连接端口发送 `egress.status`。
- [ ] **Step 3:** 确认失败。
- [ ] **Step 4:** 实现。`index.ts` 创建单例：`freeze` 和 `thaw` 通过 `sessions.get(id)?.setEgressFrozen(…)` 实现，`publish` 遍历所有 server 端口发送；`start()` 放在 runtime 就绪后，不阻塞首帧。会话生成处：
  - 用 `egressFamilyForLaunch(message.profile, providerLaunch.env)` 判定服务商。
  - 把 `exitCountry` 和 `official` 传给 `ClaudeSettings.environment`，codex 也调用它。
  - `PtySession.create` 之后立即处理 `bornFrozen`，不等首个输出。
  - 写入路径上的 `SessionEgressFrozenError` 按现有 storage paused 的方式静默丢弃，不弹错误。
- [ ] **Step 5:** 确认通过；`pnpm --filter @matou/runtime test` 全部通过。
- [ ] **Step 6:** Commit `feat(egress): wire monitor into runtime sessions and rpc`

### Task 9: 渲染层：状态、HUD 出口标签与检查面板

**Files:**
- Create: `apps/desktop/src/renderer/src/egress/use-egress-status.ts`、`apps/desktop/src/renderer/src/egress/egress-copy.ts`、`apps/desktop/src/renderer/src/egress/EgressChip.tsx`、`apps/desktop/src/renderer/src/egress/EgressPanel.tsx`、`apps/desktop/src/renderer/src/egress/egress.css`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`（处理 `egress.status` 消息，提供订阅）、`apps/desktop/src/renderer/src/hud/TerminalHud.tsx`（在 `ClaudeAccountsControl` 前插入 `EgressChip`）
- Test: `apps/desktop/src/renderer/src/egress/EgressChip.test.tsx`、`apps/desktop/src/renderer/src/egress/EgressPanel.test.tsx`、`apps/desktop/src/renderer/src/egress/use-egress-status.test.ts`

**Interfaces:**
- Produces:

```ts
export function useEgressStatus(): EgressStatusWire | undefined
export function countryLabel(country: string, locale: 'zh-CN' | 'en'): string   // 🇺🇸 美国 via Intl.DisplayNames + regional-indicator flag
export function checkCopy(check: EgressCheckWire, zh: boolean): { title: string; detail: string; fix?: string }
export function EgressChip(props: { sessionId: string }): JSX.Element | null
export function EgressPanel(props: { onClose(): void }): JSX.Element
```

- [ ] **Step 1:** 写测试：
  - `useEgressStatus`：先请求 `egress.status`，再按推送更新；runtime 断开重连后重新请求。
  - `EgressChip`：
    - `sessionId` 不在 `sessionFamilies` 中 → 不渲染。
    - 对应服务商为 `ok` → 文本「🇺🇸 美国 · 出口正常」。
    - `warning` → 「🇺🇸 美国 · 1 项提醒」，`data-tone="warn"`。
    - 冻结中 → 「出口异常 · 已暂停 N 个会话」，`data-tone="danger"`。
    - 已越过 → 「出口异常 · 已手动继续」。
    - `checking` → 「出口检查中…」。
    - 点击打开面板。
  - `EgressPanel`：
    - 按服务商分组列出 K1、K2、W0，再列出全局 W1–W4，每项显示结果和修复建议（文案取自 PRD 5.2）。
    - 「重新检查」发送 `egress.recheck`。
    - 规则单选发送 `egress.set-rule`。
    - 「以当前出口为准」只在 `violation === 'rule'` 时出现，发送 `egress.accept-current`。
    - 「出口保护」开关发送 `egress.set-protection`。
    - 「复制报告」写入剪贴板：IP 只保留前两段，不含会话 id。
    - 显示「检测于 HH:mm」与周期采样说明。
    - 参考链接（IP 质量、浏览器环境）用系统浏览器打开，并显示第三方会收到出口 IP 的提示。
  - 英文环境下全部文案为英文。
- [ ] **Step 2:** 确认失败。
- [ ] **Step 3:** 实现，样式沿用 HUD 现有 chip 和对话框的 token，不引入新颜色变量。
- [ ] **Step 4:** 确认通过；`pnpm --filter @matou/desktop test` 通过。
- [ ] **Step 5:** Commit `feat(egress): show exit status in hud and check panel`

### Task 10: 渲染层：冻结横幅、输入屏蔽与通知

**Files:**
- Create: `apps/desktop/src/renderer/src/egress/EgressFreezeBanner.tsx`、`apps/desktop/src/renderer/src/egress/egress-notifications.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`（在 `StorageFaultOverlay` 旁挂横幅；冻结时忽略键盘输入，终端仍可滚动、选择、复制）、通知挂接处（`NotificationProvider` 所在的 App 层）
- Test: `apps/desktop/src/renderer/src/egress/EgressFreezeBanner.test.tsx`、`apps/desktop/src/renderer/src/egress/egress-notifications.test.ts`、`apps/desktop/src/renderer/src/hierarchy/TerminalPane.test.tsx`

**Interfaces:**
- Consumes: `useEgressStatus`、`AgentNotificationStore.push`。
- Produces:

```ts
export function EgressFreezeBanner(props: { sessionId: string }): JSX.Element | null
export function egressNotification(previous: EgressStatusWire | undefined, next: EgressStatusWire, zh: boolean): AgentNotificationInput | undefined
```

- [ ] **Step 1:** 写测试：
  - 横幅：
    - 冻结且为 `mainland` → 「出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。」，带 `[重新检查]` `[仍要继续]`。
    - `rule` → 「出口与规则不符（美国 → 日本），已暂停。」。
    - 「仍要继续」先出二次确认，确认后发送 `egress.override`。
    - 解冻后 3 s 内显示「出口已恢复，已自动继续」，之后消失。
    - 横幅占据卡片顶部，不遮挡终端内容。
  - 通知：
    - `lastTransition.id` 变化且为 `frozen` → 标题「出口异常，已暂停 N 个会话」，`replacementKey: 'egress'`。
    - `resumed` → 「出口已恢复，N 个会话已继续」。
    - 同一个 transition id 不重复通知。
    - 首次加载的历史 transition 不通知。
  - TerminalPane：冻结时键入不调用 runtime write。
- [ ] **Step 2:** 确认失败。
- [ ] **Step 3:** 实现。
- [ ] **Step 4:** 确认通过。
- [ ] **Step 5:** Commit `feat(egress): freeze banner, input guard and notifications`

### Task 11: 实物验收

**Files:**
- Create: `docs/acceptance/network-egress-2026-11.md`、`docs/acceptance/evidence/network-egress-2026-11/`

- [ ] **Step 1:** 运行 `pnpm check:identifiers && pnpm typecheck && pnpm test`，全部通过。
- [ ] **Step 2:** 运行 `pnpm dev`。逐条跑 PRD 第 7 节场景 1–17，用本机 Clash 切换以下状态：关代理、规则直连 `api.anthropic.com`、只开系统代理、切节点、断网、睡眠唤醒。每条记录截图和实际结果。
- [ ] **Step 3:** 量性能：
  - 启动到首个终端可输入的时间，与开启前对比，差值 < 50 ms。
  - 空闲 10 分钟内，runtime 的 CPU 和网络请求数（约每分钟 1 次轻量请求）。
  - 冻结和解冻 20 个会话的耗时 < 500 ms。
- [ ] **Step 4:** 把矩阵（场景、预期、实际、证据、结论）写入验收文档。有未闭合项时，回到对应 Task 修复。
- [ ] **Step 5:** Commit `docs(egress): record network egress acceptance`
