import type { CatalogShape } from '../catalog'

export const appZhCN = {
  startupCheck: 'Matou 启动检查',
  updateRequired: '需要更新 Matou',
  upgradeIncomplete: '工作区升级未完成',
  startupHalted: 'Matou 已停止重复启动，原数据保持原样。',
  checking: '正在检查…',
  recheck: '重新检查'
}

export const appEn: CatalogShape<typeof appZhCN> = {
  startupCheck: 'Matou startup check',
  updateRequired: 'Matou needs an update',
  upgradeIncomplete: 'The workspace upgrade did not finish',
  startupHalted: 'Matou stopped restarting itself. Your data is unchanged.',
  checking: 'Checking…',
  recheck: 'Check again'
}
