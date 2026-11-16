import type { Readable, Writable } from 'node:stream'

const delay = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms))

// Private CDP pipe: no listening port and no Electron browser panel.
export class BrowserPipe {
  private sequence = 0
  private buffer = Buffer.alloc(0)
  private pages = new Set<string>()
  private sawPage = false
  private closed = false
  private requests = new Map<number, { resolve: (result: any) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>()
  constructor(private readonly input: Readable, private readonly output: Writable, private readonly lastPageClosed?: () => void) {
    input.on('data', this.receive)
    input.once('end', this.dispose)
    input.once('error', this.dispose)
    output.once('error', this.dispose)
  }
  private receive = (chunk: Buffer) => {
    this.buffer = Buffer.concat([this.buffer, chunk])
    if (this.buffer.length > 1024 * 1024) { this.dispose(); return }
    let end: number
    while ((end = this.buffer.indexOf(0)) !== -1) {
      const raw = this.buffer.subarray(0, end).toString('utf8')
      this.buffer = this.buffer.subarray(end + 1)
      if (!raw) continue
      try {
        const message = JSON.parse(raw)
        if (typeof message.id === 'number') {
          const request = this.requests.get(message.id)
          if (!request) continue
          clearTimeout(request.timer)
          this.requests.delete(message.id)
          if (message.error) request.reject(new Error('Browser command failed'))
          else request.resolve(message.result)
        } else if (message.method === 'Target.targetCreated' || message.method === 'Target.targetInfoChanged') {
          const info = message.params?.targetInfo
          if (info?.type === 'page') { this.pages.add(info.targetId); this.sawPage = true }
        } else if (message.method === 'Target.targetDestroyed') {
          const removed = this.pages.delete(message.params?.targetId)
          if (removed && this.sawPage && this.pages.size === 0) {
            void this.send('Browser.close').catch(() => {})
            this.lastPageClosed?.()
          }
        }
      } catch { this.dispose(); return }
    }
  }
  send(method: string, params: Record<string, unknown> = {}): Promise<any> {
    if (this.closed) return Promise.reject(new Error('Browser pipe closed'))
    const id = ++this.sequence
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.requests.delete(id); reject(new Error('Browser command timeout')) }, 10000)
      this.requests.set(id, { resolve, reject, timer })
      this.output.write(`${JSON.stringify({ id, method, params })}\0`, error => {
        if (error) this.dispose()
      })
    })
  }
  async ready(): Promise<void> {
    await this.send('Target.setDiscoverTargets', { discover: true })
    const deadline = Date.now() + 10000
    while (Date.now() < deadline) {
      const { targetInfos } = await this.send('Target.getTargets')
      const pages = targetInfos.filter((info: { type: string }) => info.type === 'page')
      for (const info of pages) { this.pages.add(info.targetId); this.sawPage = true }
      if (pages.length) {
        await this.send('Browser.getWindowForTarget', { targetId: pages[0].targetId })
        return
      }
      await delay(50)
    }
    throw new Error('Browser window did not appear')
  }
  async open(url: string): Promise<void> {
    const { targetId } = await this.send('Target.createTarget', { url })
    // Track the successful response too, in case its target event is delayed.
    this.pages.add(targetId)
    this.sawPage = true
    await this.send('Browser.getWindowForTarget', { targetId })
    await this.send('Target.activateTarget', { targetId })
  }
  hasPages(): boolean { return this.pages.size > 0 }
  dispose = () => {
    if (this.closed) return
    this.closed = true
    for (const request of this.requests.values()) { clearTimeout(request.timer); request.reject(new Error('Browser pipe closed')) }
    this.requests.clear()
    this.input.off('data', this.receive)
    // Keep error listeners attached until streams drain to avoid late EPIPE.
    this.buffer = Buffer.alloc(0)
  }
}
