import type {
  DagNodeSelection,
  DagWindowContext
} from '../shared/desktop-api'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'

export interface Rectangle {
  x: number
  y: number
  width: number
  height: number
}

export interface DagWindowAdapter {
  readonly id: string
  isDestroyed(): boolean
  show(): void
  focus(): void
  close(): void
  send(channel: string, value: unknown): void
  onReady(listener: () => void): void
  onClosed(listener: () => void): void
}

export interface DagWindowManagerDependencies {
  createWindow(input: { context: DagWindowContext; bounds: Rectangle }): DagWindowAdapter
  displayBounds(mainWindowId: string): Rectangle
  connectRuntime(window: DagWindowAdapter): void
  routeSelection(mainWindowId: string, selection: DagNodeSelection): void
  activateTargetWindow(windowId: string): boolean
}

export class DagWindowManager {
  readonly #dependencies: DagWindowManagerDependencies
  readonly #windows = new Map<string, DagWindowAdapter>()

  constructor(dependencies: DagWindowManagerDependencies) {
    this.#dependencies = dependencies
  }

  open(context: DagWindowContext): void {
    const existing = this.#windows.get(context.mainWindowId)
    if (existing && !existing.isDestroyed()) {
      existing.send(DESKTOP_CHANNELS.dagContext, context)
      existing.show()
      existing.focus()
      return
    }
    const bounds = centeredBounds(this.#dependencies.displayBounds(context.mainWindowId))
    const window = this.#dependencies.createWindow({ context, bounds })
    this.#windows.set(context.mainWindowId, window)
    window.onReady(() => {
      this.#dependencies.connectRuntime(window)
      window.send(DESKTOP_CHANNELS.dagContext, context)
      window.show()
      window.focus()
    })
    window.onClosed(() => {
      if (this.#windows.get(context.mainWindowId) === window) {
        this.#windows.delete(context.mainWindowId)
      }
    })
  }

  selectNode(selection: DagNodeSelection): void {
    const targetActivated = selection.targetWindowId
      ? this.#dependencies.activateTargetWindow(selection.targetWindowId)
      : false
    if (!targetActivated) {
      this.#dependencies.routeSelection(selection.mainWindowId, selection)
    }
    this.close(selection.mainWindowId)
  }

  close(mainWindowId: string): void {
    const window = this.#windows.get(mainWindowId)
    if (!window || window.isDestroyed()) return
    window.close()
  }
}

function centeredBounds(display: Rectangle): Rectangle {
  const width = Math.min(960, Math.max(680, display.width - 80))
  const height = Math.min(640, Math.max(480, display.height - 80))
  return {
    x: Math.round(display.x + (display.width - width) / 2),
    y: Math.round(display.y + (display.height - height) / 2),
    width,
    height
  }
}
