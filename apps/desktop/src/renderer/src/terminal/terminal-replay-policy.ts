export function replayFromSequenceForSpawn(message: {
  reattached?: boolean
  replayFromSequence?: number
}, hasReusableTerminalModel: boolean): number | undefined {
  if (!message.reattached || !Number.isSafeInteger(message.replayFromSequence)) return undefined
  if (message.replayFromSequence! < 0) return undefined
  return hasReusableTerminalModel ? message.replayFromSequence : 0
}

export function shouldRunReplayProbe(sessionId: string, e2e: boolean): boolean {
  return e2e && sessionId === 'foundation-shell'
}
