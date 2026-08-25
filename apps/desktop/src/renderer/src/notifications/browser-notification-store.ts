import { AgentNotificationStore } from './AgentNotificationStore'
import { playNotificationSound } from './notification-sound'

export const NOTIFICATION_SOUND_STORAGE_KEY = 'kc-notification-sound-enabled'

export function createBrowserNotificationStore(options: { playSound?: () => void } = {}): AgentNotificationStore {
  return new AgentNotificationStore({
    playSound: options.playSound ?? playNotificationSound,
    loadSoundEnabled: () => {
      try {
        const stored = window.localStorage.getItem(NOTIFICATION_SOUND_STORAGE_KEY)
        return stored === null ? true : stored !== 'false'
      } catch {
        return true
      }
    },
    persistSoundEnabled: (enabled) => {
      try {
        window.localStorage.setItem(NOTIFICATION_SOUND_STORAGE_KEY, enabled ? 'true' : 'false')
      } catch {
        // Keep the in-memory preference usable when storage is unavailable.
      }
    }
  })
}
