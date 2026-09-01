export type HostControlScope =
  | 'host.identify'
  | 'host.list'
  | 'terminal.read-current'
  | 'terminal.read-history'
  | 'terminal.read-commands'
  | 'terminal.send-text'
  | 'terminal.send-key'
  | 'task.status.write'
  | 'task.progress.write'
  | 'task.log.append'
  | 'task.move-to-window'

export interface HostCallerIdentity {
  runId: string
  sessionId: string
}

export type HostTargetSelector =
  | { kind: 'self' }
  | { kind: 'relative'; direction: 'left' | 'right' }
  | { kind: 'relation'; relation: 'parent' | 'child'; ordinal?: number }
  | { kind: 'sibling'; ordinal: number; projectionRevision: string }
  | { kind: 'ref'; ref: string; projectionRevision: string }
  | { kind: 'session'; sessionId: string }

export type HostListScope = 'current-level' | 'all'

export interface HostTarget {
  ref: string
  workspaceId: string
  taskId: string
  sessionId: string
  mountId?: string
  title: string
}

export type AllowedControlKey =
  | 'Enter'
  | 'Tab'
  | 'Escape'
  | 'Backspace'
  | 'Delete'
  | 'ArrowUp'
  | 'ArrowDown'
  | 'ArrowLeft'
  | 'ArrowRight'
  | 'Home'
  | 'End'
  | 'PageUp'
  | 'PageDown'
  | 'CtrlC'
  | 'CtrlD'
  | 'CtrlL'
  | 'CtrlU'
  | 'CtrlZ'

export class HostControlTargetNotReadyError extends Error {}
