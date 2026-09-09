import type { Locale } from '@matou/contracts'

import type { CatalogShape } from '../catalog'
import { appEn, appZhCN } from './app'
import { commonEn, commonZhCN } from './common'
import { dagEn, dagZhCN } from './dag'
import { hierarchyShellEn, hierarchyShellZhCN } from './hierarchyShell'
import { hierarchyTerminalEn, hierarchyTerminalZhCN } from './hierarchyTerminal'
import { hudEn, hudZhCN } from './hud'
import { notificationsEn, notificationsZhCN } from './notifications'
import { recoveryEn, recoveryZhCN } from './recovery'
import { sessionCanvasEn, sessionCanvasZhCN } from './sessionCanvas'
import { terminalEn, terminalZhCN } from './terminal'
import { updatesEn, updatesZhCN } from './updates'

export const zhCN = {
  common: commonZhCN, app: appZhCN, hierarchyShell: hierarchyShellZhCN, hierarchyTerminal: hierarchyTerminalZhCN,
  hud: hudZhCN, sessionCanvas: sessionCanvasZhCN, updates: updatesZhCN, recovery: recoveryZhCN,
  dag: dagZhCN, notifications: notificationsZhCN, terminal: terminalZhCN
}
export type Messages = CatalogShape<typeof zhCN>
export const en: Messages = {
  common: commonEn, app: appEn, hierarchyShell: hierarchyShellEn, hierarchyTerminal: hierarchyTerminalEn,
  hud: hudEn, sessionCanvas: sessionCanvasEn, updates: updatesEn, recovery: recoveryEn,
  dag: dagEn, notifications: notificationsEn, terminal: terminalEn
}
export const MESSAGES: Record<Locale, Messages> = { 'zh-CN': zhCN, en }
