import type { CatalogShape } from '../catalog'

export const storageZhCN = {
  // database-recovery-controller.ts
  recoveryCycleChanged: '数据库恢复周期已更新，本次操作已停止',
  originalDatabaseMissing: '原数据库尚未回到可检查的位置，请先恢复备份或导出恢复资料',
  recoveryCleanupPending: '数据库恢复失败，资源清理将在下一次操作前重试',
  recoveryTakenOver: '数据库恢复已由其他 Runtime 完成，本次操作已停止',

  // database-recovery-controller.ts and runtime-database-bootstrap.ts
  recoveryStateChanged: '数据库恢复状态已更新，请使用最新恢复页面重试',

  // read-only-database-export.ts
  noDatabaseFilesToExport: '没有可导出的数据库文件'
}

export const storageEn: CatalogShape<typeof storageZhCN> = {
  recoveryCycleChanged: 'The database recovery cycle changed; this operation was stopped',
  originalDatabaseMissing:
    'The original database is not back in a place that can be checked; restore a backup or export the recovery bundle first',
  recoveryCleanupPending: 'Database recovery failed; the cleanup is retried before the next operation',
  recoveryTakenOver: 'Another runtime already finished the database recovery; this operation was stopped',

  recoveryStateChanged: 'The database recovery state changed; retry from the latest recovery page',

  noDatabaseFilesToExport: 'There is no database file to export'
}
