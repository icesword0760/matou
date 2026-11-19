import { readFileSync, statSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { createHudState, foldHudEvent, observationHud } from './hud.mjs'
import { attachMtInstructions, attachMtShellEnvironment, hasManagedMtEnvironment } from './instructions.mjs'

export const name = 'matou-dsh-observer'
export const inject = ['agents']
const record = value => value !== null && typeof value === 'object' && !Array.isArray(value) ? value : {}
const text = value => typeof value === 'string' && value.length <= 4096 ? value : undefined
const idOf = agent => text(agent?.session?.id) ?? text(agent?.id)

/** Never send prompts, tool arguments/results, credentials, or the full log. */
export function observationData(type, value) {
  const data = record(value)
  const reason = record(data.reason)
  const error = record(reason.error)
  return {
    ...(Number.isSafeInteger(data.turn) ? { turn: data.turn } : {}),
    ...(text(data.id) ? { id: data.id } : {}),
    ...(text(data.callId) ? { callId: data.callId } : {}),
    ...(text(data.toolName) ? { toolName: data.toolName } : {}),
    ...(text(data.name) ? { name: data.name } : {}),
    ...(text(data.outcome) ? { outcome: data.outcome } : {}),
    ...(type === 'turn/end' && text(reason.kind) ? { reason: {
      kind: reason.kind,
      ...(reason.kind === 'error' ? { error: {
        ...(text(error.code) ? { code: error.code } : {}),
        ...(text(error.message) ? { message: error.message } : {})
      } } : {})
    } } : {})
  }
}

export function isTopLevelAgent(agent) {
  const header = record(agent?.session?.header)
  return Boolean(idOf(agent)) && header.origin !== 'subagent' &&
    !(typeof header.delegationDepth === 'number' && header.delegationDepth > 0) && !agent.parentAgent
}

function projectKey(cwd) {
  let readable = '', separator = false
  for (let i = 0; i < cwd.length; i++) {
    const char = cwd[i], code = cwd.charCodeAt(i)
    if (/[\\/:]/.test(char)) { if (!separator) readable += '-'; separator = true }
    else { readable += /^[A-Za-z0-9._-]$/.test(char) ? char : `~${code.toString(16).toUpperCase().padStart(4, '0')}`; separator = false }
  }
  return `--${(readable.replace(/^-+/, '') || 'root').slice(0, 251)}--`
}

function persisted(agent, env) {
  const id = idOf(agent), header = record(agent?.session?.header), cwd = text(header.cwd)
  if (!id || !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(id) || !cwd || !Number.isSafeInteger(header.version)) return false
  const root = env.DSH_TUI_SESSION_ROOT || join(env.DSH_HOME || join(homedir(), '.dsh'), 'sessions')
  const base = join(root, projectKey(cwd), id, `session.v${header.version}.jsonl`)
  return [base + '.zstd', base].some(path => { try { return statSync(path).isFile() && statSync(path).size > 0 } catch { return false } })
}

export function apply(ctx, config) {
  const url = new URL(config.hookUrl)
  if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1') throw new Error('Matou observer requires its local run hook')
  const agents = new Map(), requests = new Map(), states = new Map(), hudStates = new Map()
  let current, generation = 0, sequence = 0, disposed = false, sending = false, polling = false, requestNumber = 0, lastSnapshot = 0, mtEnvironmentReady = false
  const queue = [], controller = new AbortController()
  const disposers = [], instructedAgents = new WeakSet()
  const mtInstructions = hasManagedMtEnvironment(process.env, config.runId)
    ? readFileSync(new URL('../dsh-system-message.md', import.meta.url), 'utf8') : undefined
  const own = fn => { if (typeof fn === 'function') disposers.push(fn) }
  const post = async () => {
    if (sending || disposed) return
    sending = true
    try {
      while (queue.length && !disposed) {
        const payload = queue.shift()
        try {
          await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload), signal: AbortSignal.any([controller.signal, AbortSignal.timeout(1000)]) })
        } catch { /* Observation never blocks the native session. The next foreground snapshot resynchronizes. */ }
      }
    } finally { sending = false }
  }
  const foregroundId = () => {
    try {
      const path = join(homedir(), '.dsh-tui', 'inject', 'servers.json')
      if (statSync(path).size > 256 * 1024) return undefined
      const rows = JSON.parse(readFileSync(path, 'utf8'))
      if (!Array.isArray(rows)) return undefined
      const ownRows = rows.filter(row => row?.pid === process.pid && agents.has(row.sessionId))
      return ownRows.length === 1 ? ownRows[0].sessionId : undefined
    } catch { return undefined }
  }
  if (mtInstructions) ctx.inject(['shellEnv'], shellCtx => {
    const dispose = attachMtShellEnvironment(shellCtx, process.env, config.runId, agent =>
      !disposed && isTopLevelAgent(agent) && agents.get(idOf(agent)) === agent && foregroundId() === idOf(agent))
    mtEnvironmentReady = typeof dispose === 'function'
    shellCtx.effect(() => () => { mtEnvironmentReady = false; dispose?.() })
  })
  const trackAgent = agent => {
    if (!isTopLevelAgent(agent)) return
    const id = idOf(agent)
    agents.set(id, agent); hudStates.set(id, createHudState())
    if (mtInstructions && !instructedAgents.has(agent)) {
      const dispose = attachMtInstructions(agent, mtInstructions, () => !disposed && mtEnvironmentReady && foregroundId() === id)
      if (dispose) { instructedAgents.add(agent); own(dispose) }
    }
  }
  const emit = (type, data = {}, options = {}) => {
    const agent = agents.get(current)
    if (!agent || disposed || foregroundId() !== current) return
    if (type === 'ready' || type === 'agent/status') {
      const older = queue.findIndex(row => row.generation === generation && row.event_type === type)
      if (older >= 0) queue.splice(older, 1)
    }
    if (queue.length >= 128) queue.shift()
    queue.push({ dsh_observation: 1, run_id: config.runId, sequence: ++sequence,
      generation, session_id: current, foreground: true, event_type: type,
      event_data: observationData(type, data), hud: observationHud(agent, ctx, hudStates.get(current)), agent_status: text(agent.status), ...options })
    void post()
  }
  const snapshot = () => {
    const agent = agents.get(current)
    if (!agent) return
    emit('ready', {}, { ready: true, persisted: persisted(agent, process.env),
      cwd: text(agent.session?.header?.cwd) ?? process.cwd(),
      agent_status: text(agent.status), pending_count: [...requests.values()].filter(r => r.agentId === current && r.announced).length,
      turn_error: states.get(current)?.error === true })
  }
  own(ctx.on('agent/created', ({ agent }) => {
    trackAgent(agent)
    // This serial listener must not await the HTTP endpoint or native UI.
  }))
  own(ctx.on('agent/disposed', ({ agent }) => { agents.delete(idOf(agent)); states.delete(idOf(agent)); hudStates.delete(idOf(agent)) }))
  own(ctx.on('agent/status', ({ agent, status }) => {
    if (idOf(agent) === current && isTopLevelAgent(agent)) emit('agent/status', {}, { agent_status: text(status) })
  }))
  const observedTypes = new Set(['turn/start', 'turn/end', 'approval/asked', 'approval/decided', 'tool/call', 'tool/result', 'session/title', 'model/selection', 'request/header', 'request/context', 'permission/preset', 'sandbox/mode', 'approval/policy', 'plan/mode', 'working-directory/change'])
  own(ctx.on('session/event', (session, event) => {
    const id = String(session.id)
    if (!agents.has(id) || !observedTypes.has(event.type)) return
    foldHudEvent(hudStates.get(id) ?? createHudState(), event)
    if (event.type === 'turn/start') states.set(id, { error: false })
    if (event.type === 'turn/end' && record(event.data?.reason).kind === 'error') states.set(id, { error: true })
    if (id === current) emit(event.type, event.data)
  }))
  // Observe genuine UI answerer requests, not the mere presence of a tool call.
  for (const [event, type] of [['approval/request', 'approval/pending'], ['user-questions/request', 'question/pending']]) {
    own(ctx.on(event, async (request, next) => {
      const agentId = idOf(request.agent), key = `${type}:${++requestNumber}`
      const pending = { agentId, announced: false, timer: undefined }
      const timer = setTimeout(() => {
        if (!agents.has(agentId)) return
        pending.announced = true
        if (current === agentId) emit(type, { id: key, toolName: request.toolName })
      }, 100)
      timer.unref?.(); pending.timer = timer; requests.set(key, pending)
      try { return await next() }
      finally {
        clearTimeout(timer); requests.delete(key)
        if (pending.announced && current === agentId) emit(type.replace('/pending', '/resolved'), { id: key })
      }
    }))
  }
  for (const agent of ctx.get('agents')?.list?.() ?? []) trackAgent(agent)
  const poll = async () => {
    if (polling || disposed) return
    polling = true
    try {
      // A parked agent must stop owning the terminal as soon as its input endpoint closes.
      const target = foregroundId()
      if (!target) { current = undefined; return }
      if (target !== current) { current = target; generation++; snapshot(); lastSnapshot = Date.now() }
      else if (Date.now() - lastSnapshot >= 500) { snapshot(); lastSnapshot = Date.now() }
    } catch { /* No current endpoint means not ready; never guess by cwd or newest file. */ }
    finally { polling = false }
  }
  const timer = setInterval(poll, 100); timer.unref?.()
  void poll()
  own(ctx.provide('matouDshBridge', Object.freeze({ version: 1 })))
  ctx.effect(() => () => {
    disposed = true; clearInterval(timer); controller.abort(); queue.length = 0
    for (const request of requests.values()) clearTimeout(request.timer)
    for (const dispose of disposers.reverse()) dispose()
  })
}
