import { createContext, useContext, useEffect, useState, type ReactNode } from 'react'
import type { RuntimeMode } from '@matou/contracts'

import { RuntimeClient } from './RuntimeClient'

const PORT_CHANNEL = 'matou:terminal-port'
const RENDERER_READY = 'matou:renderer-ready'
const RuntimeContext = createContext<RuntimeClient | null>(null)

export function RuntimeProvider({ children }: { children: ReactNode }) {
  const [client, setClient] = useState<RuntimeClient | null>(null)

  useEffect(() => {
    let current: RuntimeClient | null = null
    let runtimeMode: RuntimeMode = 'normal'
    const unsubscribeLifecycle = window.matouDesktop?.onRuntimeLifecycle((presentation) => {
      runtimeMode = presentation.snapshot.mode
      current?.setRuntimeMode(runtimeMode)
    })
    const receivePort = (event: MessageEvent) => {
      if (event.source !== window || event.data?.type !== PORT_CHANNEL || event.ports.length !== 1) return
      const port = event.ports[0]
      if (!port) return
      if (current) current.replacePort(port)
      else {
        current = new RuntimeClient(port)
        current.setRuntimeMode(runtimeMode)
        setClient(current)
      }
    }
    window.addEventListener('message', receivePort)
    window.postMessage({ type: RENDERER_READY }, '*')
    return () => {
      window.removeEventListener('message', receivePort)
      if (typeof unsubscribeLifecycle === 'function') unsubscribeLifecycle()
    }
  }, [])

  return <RuntimeContext.Provider value={client}>{children}</RuntimeContext.Provider>
}

export function useRuntimeClient(): RuntimeClient | null {
  return useContext(RuntimeContext)
}
