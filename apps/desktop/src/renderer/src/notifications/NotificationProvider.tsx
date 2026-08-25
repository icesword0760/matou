import { createContext, useContext, useSyncExternalStore, type ReactNode } from 'react'

import { AgentNotificationStore, type AgentNotificationSnapshot } from './AgentNotificationStore'

const fallbackStore = new AgentNotificationStore()
const NotificationContext = createContext<AgentNotificationStore>(fallbackStore)

export function NotificationProvider({ store, children }: {
  store: AgentNotificationStore
  children: ReactNode
}) {
  return <NotificationContext.Provider value={store}>{children}</NotificationContext.Provider>
}

export function useNotificationStore(): AgentNotificationStore {
  return useContext(NotificationContext)
}

export function useNotificationSnapshot(): AgentNotificationSnapshot {
  const store = useNotificationStore()
  return useSyncExternalStore(
    (listener) => store.subscribe(listener),
    () => store.snapshot(),
    () => store.snapshot()
  )
}
