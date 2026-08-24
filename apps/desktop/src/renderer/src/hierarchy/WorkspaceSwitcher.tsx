import { useState } from 'react'

import { ConfirmDialog } from './ConfirmDialog'
import { EmptyWorkspaceState } from './EmptyWorkspaceState'
import { RenameDialog } from './RenameDialog'
import type { HierarchyCommands, HierarchyProjection } from './hierarchy-types'
import notificationIcon from '../assets/kooky/terminal/dark_toongzhi.svg'

export function WorkspaceSwitcher({ projection, commands }: {
  projection: HierarchyProjection; commands: HierarchyCommands
}) {
  const [open, setOpen] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const [removing, setRemoving] = useState(false)
  const active = projection.workspaces.find(({ id }) => id === projection.navigation.activeWorkspaceId)
  const chooseDirectory = async () => {
    const path = await window.matouDesktop?.selectWorkspaceDirectory()
    if (path) await commands.createWorkspace(path)
  }
  if (!active) return <EmptyWorkspaceState onCreate={() => void chooseDirectory()} />
  const pathState = projection.pathStates.find(({ workspaceId }) => workspaceId === active.id)
  return <div className="workspace-switcher project-dropdown">
    <button className="project-dropdown__trigger" aria-label="切换工作区" onClick={() => setOpen(!open)}>
      <span className="project-dropdown__trigger-content"><strong className="project-dropdown__name">{active.name}</strong>
      <svg className={`project-dropdown__chevron${open ? ' open' : ''}`} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="m6 9 6 6 6-6"/></svg></span>
      {pathState?.status === 'invalid' && <span className="workspace-invalid" title={reasonCopy(pathState.reason)}>路径失效</span>}
    </button>
    <span className="project-dropdown__notify-group"><i className="project-dropdown__divider" />
      <button className="project-dropdown__notify" aria-label="通知中心"><img src={notificationIcon} alt="" /></button></span>
    {open && <div role="menu" className="project-dropdown__panel">
      <div className="project-dropdown__header"><span>workspace</span>
        <button role="menuitem" onClick={() => void chooseDirectory()}>＋ 新增工作区</button></div>
      <div className="project-dropdown__list">
      {projection.workspaces.map((workspace) => {
        const state = projection.pathStates.find(({ workspaceId }) => workspaceId === workspace.id)
        return <div className={`project-dropdown__item${workspace.id === active.id ? ' active' : ''}`} key={workspace.id}>
          <button role="menuitem" aria-label={workspace.name}
          onClick={() => { setOpen(false); void commands.activateWorkspace(workspace.id) }}>
          <span><strong>{workspace.name}</strong><small title={workspace.rootDirectory}>{pathTail(workspace.rootDirectory)}</small></span>
          {state?.status === 'invalid' && <span title={reasonCopy(state.reason)}>路径失效</span>}
          </button>
        </div>
      })}
      </div>
      <div className="project-dropdown__footer-actions">
        <button role="menuitem" onClick={() => setRenaming(true)}>重命名</button>
        <button role="menuitem" onClick={() => { setOpen(false); setRemoving(true) }}>删除</button>
      </div>
    </div>}
    {renaming && <RenameDialog label="工作区名称" placeholder="请输入工作区名称" initialValue={active.name}
      onCancel={() => setRenaming(false)} onConfirm={(name) => {
        void commands.renameWorkspace(active.id, name); setRenaming(false)
      }} />}
    {removing && <ConfirmDialog title="提示"
      body={`删除 "${active.name}" 不会删除磁盘上的工作区目录，但该工作区下所有终端会话都会被丢弃，无法恢复。 是否继续?`}
      confirmLabel="确定" onCancel={() => setRemoving(false)} onConfirm={() => {
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
function reasonCopy(reason: string): string {
  return ({ missing: '目录不存在', 'not-directory': '路径不是目录', 'no-access': '目录访问受限', unknown: '目录状态异常' } as Record<string, string>)[reason] ?? '路径失效'
}
