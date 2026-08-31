import type { HierarchyProjection } from '../hierarchy/hierarchy-types'
import type { AgentNotification } from './AgentNotificationStore'
import { useNotificationSnapshot, useNotificationStore } from './NotificationProvider'

export function NotificationCenter({ projection, onClose, onNavigate }: {
  projection: HierarchyProjection
  onClose(): void
  onNavigate(notification: AgentNotification): void
}) {
  const store = useNotificationStore()
  const snapshot = useNotificationSnapshot()
  const notifications = [...snapshot.notifications].sort((left, right) => right.timestamp - left.timestamp)
  return <section className="notification-center" aria-label="通知中心">
    <header className="notification-center__header">
      <h2 className="notification-center__title">通知 <span className="notification-center__title-count">({notifications.length})</span></h2>
      <div className="notification-center__header-actions">
        {notifications.length > 0 && <button className="notification-center__action-btn"
          aria-label="清空通知" title="清空通知" onClick={() => store.clear()}><ClearIcon /></button>}
        <button className="notification-center__close-btn" aria-label="关闭通知中心" onClick={onClose}><CloseIcon /></button>
      </div>
    </header>
    <div className="notification-center__list">
      {notifications.length === 0
        ? <div className="notification-center__empty">
            <p className="notification-center__empty-text">暂无通知</p>
          </div>
        : <div className="notification-center__group"><div className="notification-center__items">
          {notifications.map((notification) => <article key={notification.id}
            className={`notification-item${notification.read ? '' : ' is-unread'}`}>
            <button className="notification-item__body" aria-label={`打开通知：${notification.body || notification.title}`}
              onClick={() => onNavigate(notification)}>
              <span className="notification-item__breadcrumb">
                <span className="notification-item__breadcrumb-part">{workspaceName(projection, notification.workspaceId)}</span>
                <span className="notification-item__breadcrumb-sep">/</span>
                <span className="notification-item__breadcrumb-part">{taskName(projection, notification.taskId)}</span>
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
            <button className="notification-item__dismiss" aria-label="清除此通知" title="清除此通知"
              onClick={() => store.remove(notification.id)}><CloseIcon /></button>
          </article>)}
        </div></div>}
    </div>
    <footer className="notification-center__footer">
      <label className="notification-center__sound-toggle">
        <span className="notification-center__sound-label">
          <span className={`notification-center__switch${snapshot.soundEnabled ? ' is-on' : ''}`}>
            <input type="checkbox" aria-label="通知声音" checked={snapshot.soundEnabled}
              onChange={(event) => store.setSoundEnabled(event.currentTarget.checked)} />
            <span className="notification-center__switch-track"><span className="notification-center__switch-thumb" /></span>
          </span>
          <span>通知声音</span>
        </span>
      </label>
    </footer>
  </section>
}

function workspaceName(projection: HierarchyProjection, id: string | null): string {
  return projection.workspaces.find((workspace) => workspace.id === id)?.name ?? '未知工作区'
}

function taskName(projection: HierarchyProjection, id: string | null): string {
  return projection.tasks.find((task) => task.id === id)?.title ?? '未知工作台'
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
