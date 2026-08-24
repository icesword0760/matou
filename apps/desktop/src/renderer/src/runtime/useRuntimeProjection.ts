import { useSyncExternalStore } from 'react'

import type { RuntimeProjectionStore, RuntimeProjectionView } from '../projection/RuntimeProjectionStore'

export function useRuntimeProjection(
  store: RuntimeProjectionStore,
  subscribe: (notify: () => void) => () => void
): RuntimeProjectionView {
  return useSyncExternalStore(subscribe, () => store.view())
}
