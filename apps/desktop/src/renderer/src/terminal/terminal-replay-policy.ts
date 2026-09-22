export function replayFromSequenceForSpawn(message: {
  reattached?: boolean
  replayFromSequence?: number
}, hasReusableTerminalModel: boolean, profile: 'shell' | 'claude-code' | 'codex',
lastAppliedSequence = 0): number | undefined {
  if (!message.reattached || !Number.isSafeInteger(message.replayFromSequence)) return undefined
  if (message.replayFromSequence! < 0) return undefined
  // A cached xterm model already contains every frame through its own visual
  // watermark. Runtime's replayFromSequence is the beginning of the current
  // PTY run, not the last frame painted by this model, so using it here would
  // append the visible prefix a second time after DOM virtualization.
  if (hasReusableTerminalModel && Number.isSafeInteger(lastAppliedSequence) && lastAppliedSequence > 0) {
    return lastAppliedSequence + 1
  }
  // Shell history comes from completed OSC 133 Blocks. Starting raw replay at
  // zero would resurrect input from an interrupted command after restart.
  // An opened offscreen terminal may never have consumed any output. It has
  // no reusable screen: ask for the authoritative live snapshot, not the
  // potentially enormous byte stream from the beginning of this PTY run.
  return profile === 'shell' ? message.replayFromSequence : 0
}

export function shouldRunReplayProbe(
  sessionId: string,
  e2e: boolean,
  diagnosticsProbe = false
): boolean {
  return e2e && (diagnosticsProbe || sessionId === 'foundation-shell')
}
