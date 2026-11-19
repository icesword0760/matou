const record = value => value && typeof value === 'object' && !Array.isArray(value) ? value : {}
const text = value => typeof value === 'string' && value.trim() && value.length <= 4096 ? value.trim() : undefined
const number = value => typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined
const safe = fn => { try { return fn() } catch { return undefined } }
export function createHudState() { return { active: new Map(), counts: new Map(), toolsKnown: false } }
export function foldHudEvent(state, event) {
  const data = record(event.data)
  if (event.type === 'session/title' && text(data.title)) state.title = text(data.title)
  if (event.type === 'tool/call') {
    const id = text(data.callId), name = text(data.name)
    if (id && name) { state.toolsKnown = true; state.active.set(id, name) }
  }
  if (event.type === 'tool/result') {
    const message = record(data.message), id = text(message.toolCallId) ?? text(data.callId), name = state.active.get(id)
    if (id && name) {
      state.active.delete(id); state.counts.set(name, (state.counts.get(name) ?? 0) + 1)
      state.lastTool = { name, status: message.isError === true ? 'error' : 'completed' }
    }
  }
  if (event.type === 'turn/end') state.active.clear()
}

/** Public current-selection and projection reads only; no history copying,
 * provider requests, setters, host-only Channel imports or instruction-path guesses. */
export function observationHud(agent, host, state = createHudState()) {
  const context = agent?.ctx ?? host
  const projections = safe(() => context.get('sessionProjections')) ?? safe(() => host.get('sessionProjections'))
  const values = record(safe(() => projections?.snapshot?.(agent.session, ['contextPressure', 'permissions', 'plan', 'title', 'workingDirectory']).values))
  const choice = record(safe(() => agent.model?.current?.()))
  const model = text(choice.model), effort = text(choice.reasoningEffort)
  const pressure = record(values.contextPressure), used = number(pressure.projectedTokens) ?? number(pressure.pressureTokens), window = number(pressure.contextWindow)
  const permissions = safe(() => context.get('permissionPresets')) ?? safe(() => host.get('permissionPresets'))
  const preset = text(record(values.permissions).currentValue) ?? text(safe(() => permissions?.current?.(agent.session)))
  const presetLabel = preset ? text(safe(() => permissions?.optionOf?.(preset)?.name)) ?? preset : undefined
  const plan = record(values.plan)
  const label = plan.active === true ? (presetLabel ? `plan · ${presetLabel}` : 'plan') : presetLabel
  const title = text(values.title) ?? state.title
  const cwd = text(values.workingDirectory) ?? text(agent?.session?.header?.cwd)
  return {
    ...(model ? { model } : {}), ...(model && effort ? { effort } : {}),
    ...(used !== undefined && window !== undefined && window > 0 ? { contextPercent: Math.min(100, used / window * 100), contextWindowSize: window } : {}),
    ...(label ? { providerPermissionLabel: label } : {}), ...(title ? { title } : {}), ...(cwd ? { cwd } : {}),
    ...(state.toolsKnown ? { runningTools: [...state.active.values()].slice(0, 128).map(name => ({ name })),
      toolCounts: [...state.counts].slice(0, 128).map(([name, count]) => ({ name, count })), ...(state.lastTool ? { lastTool: state.lastTool } : {}) } : {})
  }
}
