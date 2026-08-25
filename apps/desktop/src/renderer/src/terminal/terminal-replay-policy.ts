export function replayFromSequenceForSpawn(message: {
  reattached?: boolean
  replayFromSequence?: number
}): number | undefined {
  if (!message.reattached || !Number.isSafeInteger(message.replayFromSequence)) return undefined
  return message.replayFromSequence! >= 0 ? message.replayFromSequence : undefined
}

export function shouldRunReplayProbe(sessionId: string, e2e: boolean): boolean {
  return e2e && sessionId === 'foundation-shell'
}
