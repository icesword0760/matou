import { useState } from 'react'

import { ConfirmDialog } from './ConfirmDialog'
import { EmptyWorkspaceState } from './EmptyWorkspaceState'
import { RenameDialog } from './RenameDialog'
import type { HierarchyCommands, HierarchyProjection } from './hierarchy-types'
import { useMessages } from '../i18n/LocaleProvider'
import { useNotificationSnapshot, useNotificationStore } from '../notifications/NotificationProvider'
import { AppIcon } from '../ui/AppIcon'

export function WorkspaceSwitcher({
  projection, commands, notificationCenterOpen = false, onNotificationToggle, onWorkspaceMenuOpen
}: {
  projection: HierarchyProjection; commands: HierarchyCommands
  notificationCenterOpen?: boolean
  onNotificationToggle?(): void
  onWorkspaceMenuOpen?(): void
}) {
  const shell = useMessages().hierarchyShell
  const m = shell.workspaceSwitcher
  const reasonCopy = (reason: string) =>
    (m.pathReason as Record<string, string>)[reason] ?? shell.pathInvalid
  const [open, setOpen] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const [removing, setRemoving] = useState(false)
  const notificationStore = useNotificationStore()
  const notificationSnapshot = useNotificationSnapshot()
  const active = projection.workspaces.find(({ id }) => id === projection.navigation.activeWorkspaceId)
  const chooseDirectory = async () => {
    const path = await window.matouDesktop?.selectWorkspaceDirectory()
    if (path) await commands.createWorkspace(path)
  }
  if (!active) return <EmptyWorkspaceState onCreate={() => void chooseDirectory()} />
  const pathState = projection.pathStates.find(({ workspaceId }) => workspaceId === active.id)
  return <div className="workspace-switcher project-dropdown" data-workspace-id={active.id}>
    <button className="project-dropdown__trigger" aria-label={m.trigger} onClick={() => {
      const next = !open
      setOpen(next)
      if (next) onWorkspaceMenuOpen?.()
    }}>
      <span className="project-dropdown__trigger-content"><strong className="project-dropdown__name">{active.name}</strong>
      <svg className={`project-dropdown__chevron${open ? ' open' : ''}`} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="m6 9 6 6 6-6"/></svg></span>
      {pathState?.status === 'invalid' && <span className="workspace-invalid" title={reasonCopy(pathState.reason)}>{shell.pathInvalid}</span>}
    </button>
    <span className="project-dropdown__notify-group"><i className="project-dropdown__divider" />
      <button className="project-dropdown__notify" aria-label={shell.notificationCenter} aria-expanded={notificationCenterOpen}
        onClick={(event) => { event.stopPropagation(); setOpen(false); onNotificationToggle?.() }}>
        <AppIcon name="bell" />
        {notificationSnapshot.unreadCount > 0 && <span className="project-dropdown__notify-dot" aria-hidden="true" />}
      </button></span>
    {open && <div role="menu" className="project-dropdown__panel">
      <div className="project-dropdown__header"><span>workspace</span>
        <button role="menuitem" onClick={() => void chooseDirectory()}>{m.addWorkspace}</button></div>
      <div className="project-dropdown__list">
      {projection.workspaces.map((workspace) => {
        const state = projection.pathStates.find(({ workspaceId }) => workspaceId === workspace.id)
        return <div className={`project-dropdown__item${workspace.id === active.id ? ' active' : ''}`} key={workspace.id}>
          {notificationStore.unreadForWorkspace(workspace.id) > 0 && <span className="project-dropdown__item-dot" />}
          <button role="menuitem" aria-label={workspace.name}
          onClick={() => {
            notificationStore.markWorkspaceRead(workspace.id)
            setOpen(false)
            void commands.activateWorkspace(workspace.id)
          }}>
          <span><strong>{workspace.name}</strong><small title={workspace.rootDirectory}>{pathTail(workspace.rootDirectory)}</small></span>
          {state?.status === 'invalid' && <span title={reasonCopy(state.reason)}>{shell.pathInvalid}</span>}
          </button>
        </div>
      })}
      </div>
      <div className="project-dropdown__footer-actions">
        <button role="menuitem" onClick={() => setRenaming(true)}>{m.rename}</button>
        <button role="menuitem" onClick={() => { setOpen(false); setRemoving(true) }}>{m.delete}</button>
      </div>
    </div>}
    {renaming && <RenameDialog label={m.workspaceName} placeholder={m.workspaceNamePlaceholder} initialValue={active.name}
      onCancel={() => setRenaming(false)} onConfirm={(name) => {
        void commands.renameWorkspace(active.id, name); setRenaming(false)
      }} />}
    {removing && <ConfirmDialog title={shell.notice}
      body={m.removeBody(active.name)}
      confirmLabel={shell.confirmOk} onCancel={() => setRemoving(false)} onConfirm={() => {
        setRemoving(false)
        void Promise.resolve(commands.removeWorkspace(active.id)).catch(NOOP)
      }} />}
  </div>
}

function NOOP(): void {}

function pathTail(path: string): string {
  const parts = path.split('/').filter(Boolean)
  return parts.length <= 2 ? path : `…/${parts.slice(-2).join('/')}`
}
