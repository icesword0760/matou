import type { Locale } from '@matou/contracts'

export interface MainMessages {
  showApp: string
  quit: string
  language: string
  followSystem: string
  chinese: string
  english: string
  restartHint: string
  dagWindowTitle: string
  updateDownloaderMissing: string
  updateDmgMissing: string
  updateCheckFailed: string
  updateDownloadHttp: (status: number) => string
  updateEmptyBody: string
  updateChecksumMismatch: string
  recoveryCycleChanged: string
  recoveryExitedDuringOperation: string
  recoveryOperationFailed: string
}

const ZH_CN: MainMessages = {
  showApp: '显示码头',
  quit: '退出',
  language: '语言',
  followSystem: '跟随系统',
  chinese: '中文',
  english: 'English',
  restartHint: '运行时消息在重启后切换',
  dagWindowTitle: '码头 · 会话 DAG',
  updateDownloaderMissing: '应用内下载器尚未初始化',
  updateDmgMissing: '已下载的 DMG 路径不存在',
  updateCheckFailed: '更新检查失败',
  updateDownloadHttp: (status) => `更新文件下载失败（HTTP ${status}）`,
  updateEmptyBody: '更新服务器没有返回文件内容',
  updateChecksumMismatch: '更新文件完整性校验失败',
  recoveryCycleChanged: '数据库恢复周期已更新，本次操作已停止',
  recoveryExitedDuringOperation: '数据库恢复操作未完成：Runtime 在恢复操作期间退出',
  recoveryOperationFailed: '数据库恢复操作失败'
}

const EN: MainMessages = {
  showApp: 'Show Matou',
  quit: 'Quit',
  language: 'Language',
  followSystem: 'Follow system',
  chinese: '中文',
  english: 'English',
  restartHint: 'Runtime messages switch after a restart',
  dagWindowTitle: 'Matou · Session DAG',
  updateDownloaderMissing: 'The in-app downloader has not been initialised',
  updateDmgMissing: 'The downloaded DMG path does not exist',
  updateCheckFailed: 'Update check failed',
  updateDownloadHttp: (status) => `Update download failed (HTTP ${status})`,
  updateEmptyBody: 'The update server returned no file body',
  updateChecksumMismatch: 'Update file integrity check failed',
  recoveryCycleChanged: 'The database recovery cycle changed; this operation was stopped',
  recoveryExitedDuringOperation: 'Database recovery did not finish: the runtime exited during the operation',
  recoveryOperationFailed: 'Database recovery operation failed'
}

export function mainMessages(locale: Locale): MainMessages {
  return locale === 'en' ? EN : ZH_CN
}
