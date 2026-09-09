import type { CatalogShape } from '../catalog'

export const shellHistoryZhCN = {
  /** Written into the terminal between the replayed history and the live shell. */
  restoredDivider: '──────── 会话已恢复 ────────'
}

export const shellHistoryEn: CatalogShape<typeof shellHistoryZhCN> = {
  restoredDivider: '──────── Session restored ────────'
}
