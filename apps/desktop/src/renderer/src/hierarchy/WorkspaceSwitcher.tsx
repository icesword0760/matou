import { useState } from 'react'

import { ConfirmDialog } from './ConfirmDialog'
import { EmptyWorkspaceState } from './EmptyWorkspaceState'
import { RenameDialog } from './RenameDialog'
import type { HierarchyCommands, HierarchyProjection } from './hierarchy-types'

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
  return <div className="workspace-switcher">
    <button aria-label="切换工作区" onClick={() => setOpen(!open)}>
      <strong>{active.name}</strong>
      <span className="workspace-path" title={active.rootDirectory}>{pathTail(active.rootDirectory)}</span>
      {pathState?.status === 'invalid' && <span title={reasonCopy(pathState.reason)}>路径失效</span>}
    </button>
    {open && <div role="menu">
      {projection.workspaces.map((workspace) => {
        const state = projection.pathStates.find(({ workspaceId }) => workspaceId === workspace.id)
        return <button role="menuitem" key={workspace.id}
          onClick={() => { setOpen(false); void commands.activateWorkspace(workspace.id) }}>
          <span>{workspace.name}</span>
          {state?.status === 'invalid' && <span title={reasonCopy(state.reason)}>路径失效</span>}
        </button>
      })}
      <button role="menuitem" onClick={() => void chooseDirectory()}>新建工作区</button>
      <button role="menuitem" onClick={() => setRenaming(true)}>重命名</button>
      <button role="menuitem" onClick={() => { setOpen(false); setRemoving(true) }}>移出工作区</button>
    </div>}
    {renaming && <RenameDialog label="工作区名称" initialValue={active.name}
      onCancel={() => setRenaming(false)} onConfirm={(name) => {
        void commands.renameWorkspace(active.id, name); setRenaming(false)
      }} />}
    {removing && <ConfirmDialog title="删除工作区"
      body={`删除“${active.name}”不会删除磁盘上的工作区目录，但该工作区下所有终端会话都会被丢弃，无法恢复。是否继续？`}
      confirmLabel="确认删除" onCancel={() => setRemoving(false)} onConfirm={() => {
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
