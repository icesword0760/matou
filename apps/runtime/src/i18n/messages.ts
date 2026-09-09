import type { Locale } from '@matou/contracts'

import type { CatalogShape } from './catalog'
import { runtimeLocale } from './locale'
import { controlEn, controlZhCN } from './messages/control'
import { gitEn, gitZhCN } from './messages/git'
import { hierarchyEn, hierarchyZhCN } from './messages/hierarchy'
import { providerConfigEn, providerConfigZhCN } from './messages/providerConfig'
import { serverEn, serverZhCN } from './messages/server'
import { sessionEn, sessionZhCN } from './messages/session'
import { sessionCanvasEn, sessionCanvasZhCN } from './messages/sessionCanvas'
import { shellHistoryEn, shellHistoryZhCN } from './messages/shellHistory'
import { storageEn, storageZhCN } from './messages/storage'

export const zhCN = {
  control: controlZhCN, server: serverZhCN, sessionCanvas: sessionCanvasZhCN, providerConfig: providerConfigZhCN,
  hierarchy: hierarchyZhCN, storage: storageZhCN, git: gitZhCN, session: sessionZhCN, shellHistory: shellHistoryZhCN
}
export type RuntimeMessages = CatalogShape<typeof zhCN>
export const en: RuntimeMessages = {
  control: controlEn, server: serverEn, sessionCanvas: sessionCanvasEn, providerConfig: providerConfigEn,
  hierarchy: hierarchyEn, storage: storageEn, git: gitEn, session: sessionEn, shellHistory: shellHistoryEn
}
const MESSAGES: Record<Locale, RuntimeMessages> = { 'zh-CN': zhCN, en }

export function runtimeMessages(): RuntimeMessages { return MESSAGES[runtimeLocale()] }
