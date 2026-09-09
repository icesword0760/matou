import type { HierarchyProjection } from '../hierarchy/hierarchy-types'
import { useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'
import type { AgentNotification } from './AgentNotificationStore'
import { useNotificationSnapshot, useNotificationStore } from './NotificationProvider'

export function NotificationCenter({ projection, onClose, onNavigate }: {
  projection: HierarchyProjection
  onClose(): void
  onNavigate(notification: AgentNotification): void
}) {
  const m = useMessages().notifications.center
  const store = useNotificationStore()
  const snapshot = useNotificationSnapshot()
  const notifications = [...snapshot.notifications].sort((left, right) => right.timestamp - left.timestamp)
  return <section className="notification-center" aria-label={m.label}>
    <header className="notification-center__header">
      <h2 className="notification-center__title">{m.title} <span className="notification-center__title-count">({notifications.length})</span></h2>
      <div className="notification-center__header-actions">
        {notifications.length > 0 && <button className="notification-center__action-btn"
          aria-label={m.clear} title={m.clear} onClick={() => store.clear()}><ClearIcon /></button>}
        <button className="notification-center__close-btn" aria-label={m.close} onClick={onClose}><CloseIcon /></button>
      </div>
    </header>
    <div className="notification-center__list">
      {notifications.length === 0
        ? <div className="notification-center__empty">
            <p className="notification-center__empty-text">{m.empty}</p>
          </div>
        : <div className="notification-center__group"><div className="notification-center__items">
          {notifications.map((notification) => <article key={notification.id}
            className={`notification-item${notification.read ? '' : ' is-unread'}`}>
            <button className="notification-item__body" aria-label={m.open(notification.body || notification.title)}
              onClick={() => onNavigate(notification)}>
              <span className="notification-item__breadcrumb">
                <span className="notification-item__breadcrumb-part">{workspaceName(m, projection, notification.workspaceId)}</span>
                <span className="notification-item__breadcrumb-sep">/</span>
                <span className="notification-item__breadcrumb-part">{taskName(m, projection, notification.taskId)}</span>
              </span>
              <span className="notification-item__title-row">
                <strong className="notification-item__title">{notification.title || 'Claude Code'}</strong>
                {notification.teamRole && <span className="notification-item__role-badge">{notification.teamRole}</span>}
                {notification.teamStatus && notification.teamStatusTone !== 'idle' &&
                  <span className={`notification-item__status-badge tone-${notification.teamStatusTone || 'default'}`}>{notification.teamStatus}</span>}
              </span>
              {notification.subtitle && <span className="notification-item__subtitle">{notification.subtitle}</span>}
              {notification.body && <span className="notification-item__content">{notification.body}</span>}
              <time className="notification-item__time" dateTime={new Date(notification.timestamp).toISOString()}>
                {formatTime(notification.timestamp)}
              </time>
            </button>
            <button className="notification-item__dismiss" aria-label={m.dismiss} title={m.dismiss}
              onClick={() => store.remove(notification.id)}><CloseIcon /></button>
          </article>)}
        </div></div>}
    </div>
    <footer className="notification-center__footer">
      <label className="notification-center__sound-toggle">
        <span className="notification-center__sound-label">
          <span className={`notification-center__switch${snapshot.soundEnabled ? ' is-on' : ''}`}>
            <input type="checkbox" aria-label={m.sound} checked={snapshot.soundEnabled}
              onChange={(event) => store.setSoundEnabled(event.currentTarget.checked)} />
            <span className="notification-center__switch-track"><span className="notification-center__switch-thumb" /></span>
          </span>
          <span>{m.sound}</span>
        </span>
      </label>
    </footer>
  </section>
}

function workspaceName(
  m: Messages['notifications']['center'],
  projection: HierarchyProjection,
  id: string | null
): string {
  return projection.workspaces.find((workspace) => workspace.id === id)?.name ?? m.unknownWorkspace
}

function taskName(
  m: Messages['notifications']['center'],
  projection: HierarchyProjection,
  id: string | null
): string {
  return projection.tasks.find((task) => task.id === id)?.title ?? m.unknownTask
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp)
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}

function CloseIcon() {
  return <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
    strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
}

function ClearIcon() {
  return <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
    strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="m19 6-1 14H6L5 6"/></svg>
}
