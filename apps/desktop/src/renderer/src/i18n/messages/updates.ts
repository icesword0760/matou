import type { CatalogShape } from '../catalog'

export const updatesZhCN = {
  /** Accessible name of the update popover. */
  popover: 'Matou 应用更新',
  /** Accessible name of the toolbar trigger; it also reports the current update stage. */
  buttonLabel: {
    idle: '应用更新',
    checking: '应用更新：正在检查',
    available: (version: string) => `应用更新：发现 ${version}`,
    downloading: (percent: number) => `应用更新：下载中 ${percent}%`,
    awaitingInstaller: '应用更新：等待打开安装包',
    awaitingInstall: '应用更新：等待安装',
    error: (title: string) => `应用更新：${title}`
  },
  header: {
    available: (version: string) => `Matou ${version} 可用`,
    downloading: '正在后台下载',
    dmgReady: 'DMG 已下载完成',
    ready: '更新已准备好',
    checking: '正在检查更新',
    upToDate: 'Matou 已是最新版本',
    stableChannel: '稳定版',
    close: '关闭更新浮层'
  },
  currentVersion: (version: string) => `当前版本 ${version}`,
  checking: '正在检查云端是否有新版本…',
  retrying: (attempt: number, max: number) => `连接波动，正在自动重试（${attempt}/${max}）…`,
  upToDate: (version: string) => `当前已是最新版本（${version}）`,
  manualDmgNotice: '当前体验包将通过应用内下载 DMG 更新。',
  activeSessions: (count: number) => `当前有 ${count} 个活动会话`,
  idleUpdateKeepsState: '空闲后更新会保留工作区、画布位置及会话恢复信息。',
  idleUpdateScheduled: '已安排：空闲后自动更新',
  remainingSeconds: (seconds: number) => `约 ${seconds} 秒`,
  downloadRate: (perSecond: string) => `${perSecond}/秒 · 可继续使用当前会话`,
  updatedTo: (version: string) => `Matou 已更新至 ${version}`,
  updatedHint: '工作空间与会话已恢复',
  actions: {
    check: '检查更新',
    downloadDmg: '下载 DMG 更新',
    recheck: '重新检查',
    download: '下载更新',
    downloadInBackground: '后台下载',
    remindLater: '稍后提醒',
    keepInBackground: '继续在后台下载',
    opening: '正在打开…',
    openDmg: '打开 DMG 安装',
    installLater: '稍后安装',
    cancelIdleUpdate: '取消空闲更新',
    updateWhenIdle: '空闲后自动更新',
    restartNow: '立即重启并更新',
    installOnQuit: '退出时安装',
    preparing: '正在准备更新…',
    restart: '重启并更新'
  },
  errorTitle: {
    check: '更新检查失败',
    download: '更新下载失败',
    verify: '安装包校验未通过',
    install: '更新安装失败'
  },
  error: {
    signatureMissing: '当前安装包缺少 Apple 发布签名，改用应用内 DMG 下载继续更新。',
    installFailed: '更新安装前的会话保存或应用退出过程出现异常，请重新尝试。',
    dnsFailed: '更新服务器域名解析失败，请检查网络或 DNS 设置。',
    timedOut: '连接更新服务器超时，请检查网络后重试。',
    offline: '当前设备尚未接入网络，请恢复网络后重试。',
    connectionRefused: '更新服务器暂时拒绝连接，请稍后重试。',
    connectionReset: '下载连接被中途断开，应用已保留当前版本，请重新尝试。',
    tlsFailed: '更新服务器的安全连接校验异常，请检查系统时间后重试。',
    httpStatus: (status: string) => `更新服务器返回 HTTP ${status}，请稍后重试。`,
    manifestInvalid: '更新信息格式异常，当前版本保持不变，请稍后重试。',
    networkUnavailable: '暂时没有连接到更新服务器，请检查网络后重试。',
    downloadInterrupted: '更新文件下载中断，请检查网络后重试。',
    unknown: '更新服务出现异常，请稍后重新检查。'
  }
}

export const updatesEn: CatalogShape<typeof updatesZhCN> = {
  popover: 'Matou app update',
  buttonLabel: {
    idle: 'App update',
    checking: 'App update: checking',
    available: (version: string) => `App update: ${version} available`,
    downloading: (percent: number) => `App update: downloading ${percent}%`,
    awaitingInstaller: 'App update: waiting to open installer',
    awaitingInstall: 'App update: waiting to install',
    error: (title: string) => `App update: ${title}`
  },
  header: {
    available: (version: string) => `Matou ${version} is available`,
    downloading: 'Downloading in background',
    dmgReady: 'DMG downloaded',
    ready: 'Update ready',
    checking: 'Checking for updates',
    upToDate: 'Matou is up to date',
    stableChannel: 'Stable',
    close: 'Close update popover'
  },
  currentVersion: (version: string) => `Current version ${version}`,
  checking: 'Checking for a newer version…',
  retrying: (attempt: number, max: number) =>
    `Connection is unstable, retrying automatically (${attempt}/${max})…`,
  upToDate: (version: string) => `Already on the latest version (${version})`,
  manualDmgNotice: 'This build updates through an in-app DMG download.',
  activeSessions: (count: number) =>
    count === 1 ? 'You have 1 active session' : `You have ${count} active sessions`,
  idleUpdateKeepsState:
    'Updating once idle keeps your workspaces, canvas positions and session recovery data.',
  idleUpdateScheduled: 'Scheduled: update once idle',
  remainingSeconds: (seconds: number) => `about ${seconds}s left`,
  downloadRate: (perSecond: string) => `${perSecond}/s · you can keep using your sessions`,
  updatedTo: (version: string) => `Matou updated to ${version}`,
  updatedHint: 'Workspaces and sessions restored',
  actions: {
    check: 'Check for updates',
    downloadDmg: 'Download DMG update',
    recheck: 'Check again',
    download: 'Download update',
    downloadInBackground: 'Download in background',
    remindLater: 'Remind me later',
    keepInBackground: 'Keep downloading in background',
    opening: 'Opening…',
    openDmg: 'Open DMG installer',
    installLater: 'Install later',
    cancelIdleUpdate: 'Cancel idle update',
    updateWhenIdle: 'Update once idle',
    restartNow: 'Install and restart now',
    installOnQuit: 'Install on quit',
    preparing: 'Preparing update…',
    restart: 'Install and restart'
  },
  errorTitle: {
    check: 'Update check failed',
    download: 'Update download failed',
    verify: 'Installer verification failed',
    install: 'Update installation failed'
  },
  error: {
    signatureMissing:
      'This installer has no Apple release signature. Matou will continue with the in-app DMG download.',
    installFailed:
      'Saving sessions or quitting the app before the install went wrong. Please try again.',
    dnsFailed: 'The update server address could not be resolved. Check your network or DNS settings.',
    timedOut: 'The connection to the update server timed out. Check your network and try again.',
    offline: 'This device is not connected to a network. Reconnect and try again.',
    connectionRefused: 'The update server refused the connection. Please try again shortly.',
    connectionReset:
      'The download connection was cut off. Matou kept the current version, please try again.',
    tlsFailed:
      'The secure connection to the update server failed verification. Check the system clock and try again.',
    httpStatus: (status: string) => `The update server returned HTTP ${status}. Please try again later.`,
    manifestInvalid:
      'The update manifest is malformed. The current version is unchanged, please try again later.',
    networkUnavailable: 'Matou could not reach the update server. Check your network and try again.',
    downloadInterrupted: 'The update download was interrupted. Check your network and try again.',
    unknown: 'The update service ran into a problem. Please check again later.'
  }
}
