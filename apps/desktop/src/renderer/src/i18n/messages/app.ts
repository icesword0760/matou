import type { CatalogShape } from '../catalog'

export const appZhCN = {
  openingWorkspace: '正在打开工作区…',
  restoringWorkspace: '正在恢复上次的工作状态，本地记录较多时可能需要更长时间。',
  startupCheck: 'Matou 启动检查',
  updateRequired: '需要更新 Matou',
  upgradeIncomplete: '工作区升级未完成',
  startupHalted: 'Matou 已停止重复启动，原数据保持原样。',
  checking: '正在检查…',
  recheck: '重新检查'
}

export const appEn: CatalogShape<typeof appZhCN> = {
  openingWorkspace: 'Opening your workspace…',
  restoringWorkspace: 'Restoring your previous work. This may take longer with a large local history.',
  startupCheck: 'Matou startup check',
  updateRequired: 'Matou needs an update',
  upgradeIncomplete: 'The workspace upgrade did not finish',
  startupHalted: 'Matou stopped restarting itself. Your data is unchanged.',
  checking: 'Checking…',
  recheck: 'Check again'
}
