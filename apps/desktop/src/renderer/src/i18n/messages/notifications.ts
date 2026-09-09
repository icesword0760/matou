import type { CatalogShape } from '../catalog'

export const notificationsZhCN = {
  // NotificationCenter
  center: {
    label: '通知中心',
    title: '通知',
    clear: '清空通知',
    close: '关闭通知中心',
    empty: '暂无通知',
    open: (text: string) => `打开通知：${text}`,
    dismiss: '清除此通知',
    sound: '通知声音',
    unknownWorkspace: '未知工作区',
    unknownTask: '未知工作台'
  },

  /** Body of an OSC notification that carried no readable text. */
  fallbackBody: '终端通知'
}

export const notificationsEn: CatalogShape<typeof notificationsZhCN> = {
  center: {
    label: 'Notification center',
    title: 'Notifications',
    clear: 'Clear all notifications',
    close: 'Close notification center',
    empty: 'No notifications',
    open: (text: string) => `Open notification: ${text}`,
    dismiss: 'Dismiss this notification',
    sound: 'Notification sound',
    unknownWorkspace: 'Unknown workspace',
    unknownTask: 'Unknown task'
  },

  fallbackBody: 'Terminal notification'
}
