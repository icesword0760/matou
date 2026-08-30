import type { SessionWorkStatus } from '@matou/domain'

import type { ProviderNotificationEvent } from './provider-notification-event'

export function nextProviderWorkStatus(
  current: SessionWorkStatus | undefined,
  eventType: ProviderNotificationEvent['eventType']
): SessionWorkStatus {
  // A Stop hook is emitted when Claude gives control back to its input box,
  // including after a failed final network attempt. Keep a detected failure
  // sticky; explicit user input or the retry command changes it to running.
  if (current === 'error') return 'error'
  if (eventType === 'error') return 'error'
  if (eventType === 'completed') return 'idle'
  return 'needs-input'
}
