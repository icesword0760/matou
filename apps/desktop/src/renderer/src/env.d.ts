/// <reference types="vite/client" />

import type { MatouDesktopApi } from '../../shared/desktop-api'
import type { AgentNotificationInput } from './notifications/AgentNotificationStore'

declare global {
  interface Window {
    matouDesktop: MatouDesktopApi
    matouE2e?: {
      pushNotification(input: AgentNotificationInput): void
      moveTaskToWindow(input: {
        migrationId: string
        taskId: string
        sourceWindowId: string
        targetWindowId: string
      }): Promise<void>
    }
  }
}
