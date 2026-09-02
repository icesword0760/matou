export function replayFromSequenceForSpawn(message: {
  reattached?: boolean
  replayFromSequence?: number
}, hasReusableTerminalModel: boolean, profile: 'shell' | 'claude-code' | 'codex'): number | undefined {
  if (!message.reattached || !Number.isSafeInteger(message.replayFromSequence)) return undefined
  if (message.replayFromSequence! < 0) return undefined
  // Shell history comes from completed OSC 133 Blocks. Starting raw replay at
  // zero would resurrect input from an interrupted command after restart.
  return hasReusableTerminalModel || profile === 'shell' ? message.replayFromSequence : 0
}

export function shouldRunReplayProbe(sessionId: string, e2e: boolean): boolean {
  return e2e && sessionId === 'foundation-shell'
}
