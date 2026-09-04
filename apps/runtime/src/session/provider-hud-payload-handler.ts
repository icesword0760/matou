import type { ProviderCli } from '@matou/contracts'

import { providerTranscriptPath } from './provider-hook-server'
import { SessionHudRegistry } from './session-hud-registry'

export interface ProviderHudPayloadEvent {
  runId: string
  sessionId: string
  provider: ProviderCli
  payload: Record<string, unknown>
}

export function createProviderHudPayloadHandler(options: {
  hud: SessionHudRegistry
  currentRunId(sessionId: string): string | undefined
  publish(sessionId: string): void
  reportError(error: unknown): void
}): (event: ProviderHudPayloadEvent) => void {
  return ({ runId, sessionId, payload }) => {
    const ownsRun = () => options.currentRunId(sessionId) === runId
    if (!ownsRun()) return
    options.hud.ingestProvider(sessionId, payload)
    options.publish(sessionId)
    const transcriptPath = providerTranscriptPath(payload)
    if (!transcriptPath) return
    void options.hud.refreshTranscript(sessionId, transcriptPath, {
      runId,
      currentRunId: () => options.currentRunId(sessionId)
    }).then((changed) => {
      if (changed && ownsRun()) options.publish(sessionId)
    }).catch(options.reportError)
  }
}
