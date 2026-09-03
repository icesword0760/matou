import type { HostActionMethod } from './host-action-types'

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
  | HostActionMethod

export interface HostCallerIdentity {
  runId: string
  sessionId: string
}

export interface HostControlErrorDetails {
  candidates: ReadonlyArray<{ readonly humanPath: string }>
}

export type HostTargetSelector =
  | { kind: 'self' }
  | { kind: 'relative'; direction: 'left' | 'right' }
  | { kind: 'relation'; relation: 'parent' | 'child'; ordinal?: number }
  | { kind: 'sibling'; ordinal: number; projectionRevision: string }
  | { kind: 'ref'; ref: string; projectionRevision: string }
  | { kind: 'session'; sessionId: string }

export type HostListScope = 'current-level' | 'all'

export interface HostTargetEnvironment {
  executionContextRef: string
  mode: 'directory' | 'git-checkout' | 'git-worktree'
  branch?: string
  worktreeRef?: string
}

export interface HostTarget {
  ref: string
  workspaceId: string
  taskId: string
  sessionId: string
  mountId?: string
  title: string
  profile: 'shell' | 'claude-code' | 'codex'
  cwd: string
  workStatus: string
  environment: HostTargetEnvironment
  window: {
    id: string
    kind: 'main' | 'detached-terminal'
    ordinal: number
  }
  workspace: { id: string; name: string; ordinal: number }
  task: { id: string; name: string; ordinal: number }
  canvas: { id: string; name: string; ordinal: number }
  session: { id: string; ordinal: number; detached: boolean }
  dag: { depth: number; parentRef?: string; childRefs: string[]; siblingRefs: string[] }
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
export class HostControlTargetNotFoundError extends Error {}
