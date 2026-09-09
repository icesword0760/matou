import type { CatalogShape } from '../catalog'

export const hierarchyShellZhCN = {
  emptyWorkspace: { title: '还没有工作区', hint: '选择一个本地目录开始工作。', create: '新建工作区' }
}
export const hierarchyShellEn: CatalogShape<typeof hierarchyShellZhCN> = {
  emptyWorkspace: { title: 'No workspace yet', hint: 'Pick a local directory to start working.', create: 'New workspace' }
}
