/** Markers belonging to the agent that launched Matou, not its independent cards.
 * In particular, Claude's child-session marker suppresses transcript persistence.
 * Keep credentials, provider configuration, proxies and user privacy settings.
 */
export const CALLER_SESSION_MARKERS = [
  'CLAUDECODE',
  'CLAUDE_CODE_CHILD_SESSION',
  'CLAUDE_CODE_SESSION_ID',
  'CLAUDE_CODE_SESSION_ATTENDED',
  'CLAUDE_PID',
  'CLAUDE_CODE_ENTRYPOINT',
  'CLAUDE_CODE_SESSION_NAME',
  'CLAUDE_CODE_SESSION_KIND',
  'CLAUDE_BG_SOURCE',
  'CLAUDE_BG_BACKEND',
  'CLAUDE_BG_RENDEZVOUS_SOCK',
  'CLAUDE_CODE_RESUME_PROMPT',
  'CLAUDE_CODE_RESUME_INTERRUPTED_TURN'
] as const

export function independentSessionEnvironment(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const result = { ...env }
  for (const key of CALLER_SESSION_MARKERS) delete result[key]
  return result
}
