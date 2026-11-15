/** A native agent-scoped system section. Never interpolate run credentials. */
export function hasManagedMtEnvironment(env, runId) {
  return typeof runId === 'string' && runId.length > 0 && env.MATOU_CONTROL_PROTOCOL === '1' && env.MATOU_CONTROL_CALLER_RUN === runId &&
    ['MATOU_CONTROL_ENDPOINT', 'MATOU_CONTROL_TOKEN', 'MATOU_CONTROL_CALLER_SESSION', 'MATOU_CONTROL_NODE_EXECUTABLE']
      .every(key => typeof env[key] === 'string' && env[key].length > 0)
}

export function attachMtInstructions(agent, markdown, ownsForeground) {
  if (!agent?.ctx || !markdown?.trim()) return undefined
  const prompt = agent.ctx.get('systemPrompt')
  if (typeof prompt?.section !== 'function') return undefined
  return prompt.section({
    name: 'matou:host-control', order: 3500, interpolate: false,
    // Scoped layers can inherit into native subagents. Only the exact foreground
    // agent may receive this Matou card's capability and collaboration rules.
    text: context => context?.scope === agent && ownsForeground() ? markdown : ''
  })
}

/** Deliberate official post-scrub forwarding for this exact foreground call. */
export function attachMtShellEnvironment(ctx, env, runId, ownsAgent) {
  const registry = ctx.get('shellEnv')
  if (typeof registry?.register !== 'function' || !hasManagedMtEnvironment(env, runId)) return undefined
  return registry.register({
    name: 'matou:host-control',
    variables: { DSH_MATOU_CONTROL_TOKEN: { description: 'Private current-session Matou control identity for mt; never print it.' } },
    resolve: execution => hasManagedMtEnvironment(env, runId) && ownsAgent(execution?.agent)
      ? { DSH_MATOU_CONTROL_TOKEN: env.MATOU_CONTROL_TOKEN } : {}
  })
}
