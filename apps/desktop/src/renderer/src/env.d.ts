/// <reference types="vite/client" />

import type { MatouDesktopApi } from '../../shared/desktop-api'

declare global {
  interface Window {
    matouDesktop: MatouDesktopApi
  }
}
