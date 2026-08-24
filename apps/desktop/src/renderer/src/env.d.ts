/// <reference types="vite/client" />

import type { MatouDesktopApi } from '../../shared/desktop-api'

declare global {
  interface Window {
    matouDesktop: MatouDesktopApi
    matouE2e?: {
      moveTaskToWindow(input: {
        migrationId: string
        taskId: string
        sourceWindowId: string
        targetWindowId: string
      }): Promise<void>
    }
  }
}
