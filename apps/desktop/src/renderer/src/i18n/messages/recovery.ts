import type { CatalogShape } from '../catalog'

export const recoveryZhCN = {
  page: {
    eyebrow: 'Matou 数据恢复',
    title: '数据库需要恢复',
    ownershipTitle: '数据库占用状态需要处理',
    ownershipDescription:
      '数据库占用记录或接管状态异常，原数据库仍保留在原位置。处理前不会将其当作损坏文件移动。',
    walDescription:
      '数据库日志状态不完整。Matou 已保留原数据库和日志文件，请选择备份恢复或导出资料。',
    integrityDescription: '数据库完整性检查未通过。Matou 已保留原文件，不会直接进入全新空工作区。',
    working: '正在处理数据库恢复，请保持 Matou 开启…',
    exportedTo: (path: string) => `恢复资料已导出到 ${path}`,
    backups: '可用备份',
    backupCount: (count: number) => `${count} 份`,
    noBackups: '暂未找到通过完整性校验的备份。',
    schemaVersion: (version: number) => `数据版本 ${version}`,
    restoring: '正在恢复…',
    restoreSelected: '恢复所选备份',
    checking: '正在检查…',
    recheckDatabase: '重新检查数据库',
    exporting: '正在导出…',
    exportBundle: '导出恢复资料',
    startEmpty: '创建全新空数据库',
    startEmptyHint: '此入口只在你明确确认后执行；现有隔离文件和备份继续保留。',
    confirmEmptyLabel: '确认创建全新空数据库',
    confirmEmptyTitle: '确认创建全新空数据库？',
    confirmEmptyBody:
      'Matou 将显示一个全新的空工作区。当前损坏或异常文件和备份仍会保留，便于后续导出与排查。',
    back: '返回',
    confirmEmpty: '确认创建空数据库'
  },
  readOnlyBanner: {
    explanation: '现有工作空间、事项和会话仍可浏览、搜索与复制；可能改动数据的操作已暂停。',
    searchTerminal: '搜索当前终端',
    exporting: '正在导出…',
    exportDatabase: '导出数据库资料',
    exportedTo: (path: string) => `数据库资料已导出到 ${path}`,
    exported: '数据库资料已导出',
    exportFailed: (reason: string) => `导出失败：${reason}`
  }
}

export const recoveryEn: CatalogShape<typeof recoveryZhCN> = {
  page: {
    eyebrow: 'Matou data recovery',
    title: 'The database needs recovery',
    ownershipTitle: 'The database ownership state needs attention',
    ownershipDescription:
      'The database ownership record or handover state is inconsistent. The original database is still in place and will not be moved aside as a damaged file before you act.',
    walDescription:
      'The database log is incomplete. Matou kept the original database and log files, so you can restore a backup or export your data.',
    integrityDescription:
      'The database failed its integrity check. Matou kept the original files and will not start a blank workspace on its own.',
    working: 'Recovering the database. Keep Matou open…',
    exportedTo: (path: string) => `Recovery data exported to ${path}`,
    backups: 'Available backups',
    backupCount: (count: number) => (count === 1 ? '1 backup' : `${count} backups`),
    noBackups: 'No backup has passed the integrity check yet.',
    schemaVersion: (version: number) => `Schema version ${version}`,
    restoring: 'Restoring…',
    restoreSelected: 'Restore the selected backup',
    checking: 'Checking…',
    recheckDatabase: 'Check the database again',
    exporting: 'Exporting…',
    exportBundle: 'Export recovery data',
    startEmpty: 'Create a new empty database',
    startEmptyHint:
      'This runs only after you confirm it explicitly. The quarantined files and the backups are kept.',
    confirmEmptyLabel: 'Confirm creating a new empty database',
    confirmEmptyTitle: 'Create a new empty database?',
    confirmEmptyBody:
      'Matou will show a brand-new empty workspace. The damaged files and the backups are kept so you can export and investigate them later.',
    back: 'Back',
    confirmEmpty: 'Create the empty database'
  },
  readOnlyBanner: {
    explanation:
      'Existing workspaces, tasks and sessions can still be browsed, searched and copied. Anything that would change data is paused.',
    searchTerminal: 'Search this terminal',
    exporting: 'Exporting…',
    exportDatabase: 'Export the database',
    exportedTo: (path: string) => `Database exported to ${path}`,
    exported: 'Database exported',
    exportFailed: (reason: string) => `Export failed: ${reason}`
  }
}
