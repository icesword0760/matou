// Isolated process-boundary fixture, not a replacement for the product runtime.
import type { ParentPort } from 'electron'
import { ModelRouterManager } from '../../apps/runtime/src/model-router/manager'
import { RouterHostBridge } from '../../apps/runtime/src/model-router/host-bridge'
const parentPort = (process as NodeJS.Process & { parentPort?: ParentPort }).parentPort
if (!parentPort || !process.env.MATOU_DATA_DIR) throw new Error('Missing isolated runtime context')
const bridge = new RouterHostBridge(message => parentPort.postMessage(message))
const manager = new ModelRouterManager({ dataRoot: process.env.MATOU_DATA_DIR, control: bridge })
const ready = manager.openSettings('fixture-connection')
const authorization = bridge.openAuthorizationUrl('codex', 'https://auth.openai.com/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback')
parentPort.on('message', event => {
  if (bridge.receive(event.data)) return
  if (event.data?.type === 'runtime.scale-metrics-request') {
    void Promise.all([ready, authorization]).then(([lease]) => parentPort.postMessage({
      type: 'runtime.scale-metrics-result', requestId: event.data.requestId,
      runtimePid: process.pid, ptyCount: 0, ptyPids: [lease.connection.pid],
      statementCount: 0, statementProfile: [], eventLoopDelayP99Ms: 0,
      eventLoopDelayMaxMs: 0, maxUnackedBytes: 0, retainedDurabilityBytes: 0,
      heapUsedBytes: 0, externalBytes: 0, arrayBufferBytes: 0
    }))
  }
})
process.once('SIGTERM', () => { void manager.close().then(() => process.exit(0)) })
void ready.catch(error => { console.error(error.message); process.exit(1) })
