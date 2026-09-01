import { cpus, freemem, hostname, platform, release, totalmem } from 'node:os'

import type { MatouFixture } from '../matou-fixture'

export interface ScaleSample {
  name: string
  count: number
  warmupRuns: number
  measuredRuns: number
  p50: number
  p95: number
  max: number
  electronPid: number
  rendererPid: number
  runtimePid: number
  rendererRssMb: number
  runtimeRssMb: number
  ptyCount: number
  ptyPids: number[]
  domNodes: number
  statementCount: number
  longTaskCount: number
  longTaskP95: number
  longTaskMax: number
  machine: {
    hostname: string
    platform: string
    release: string
    cpuModel: string
    cpuCount: number
    totalMemoryMb: number
    freeMemoryMb: number
  }
}

export interface CollectScaleSampleOptions {
  name: string
  minimumFrameCount?: number
  warmupRuns?: number
  measuredRuns?: number
}

interface RuntimeScaleMetrics {
  runtimePid: number
  ptyCount: number
  ptyPids: number[]
  statementCount: number
}

interface ProcessScaleMetrics {
  electronPid: number
  rendererPid: number
  rendererRssMb: number
  runtimeRssMb: number
}

interface BrowserFrameSample {
  frameDurations: number[]
  longTasks: number[]
  domNodes: number
}

export async function collectScaleSample(
  fixture: MatouFixture,
  options: CollectScaleSampleOptions
): Promise<ScaleSample> {
  const minimumFrameCount = options.minimumFrameCount ?? 120
  const warmupRuns = options.warmupRuns ?? 2
  const measuredRuns = options.measuredRuns ?? 5
  assertPositiveInteger(minimumFrameCount, 'minimumFrameCount')
  assertNonNegativeInteger(warmupRuns, 'warmupRuns')
  assertPositiveInteger(measuredRuns, 'measuredRuns')

  for (let run = 0; run < warmupRuns; run += 1) {
    await collectBrowserFrames(fixture, minimumFrameCount)
  }
  await readRuntimeMetrics(fixture, true)

  const frameDurations: number[] = []
  const longTasks: number[] = []
  let domNodes = 0
  for (let run = 0; run < measuredRuns; run += 1) {
    const sample = await collectBrowserFrames(fixture, minimumFrameCount)
    frameDurations.push(...sample.frameDurations)
    longTasks.push(...sample.longTasks)
    domNodes = Math.max(domNodes, sample.domNodes)
  }
  const runtime = await readRuntimeMetrics(fixture, false)
  const processMetrics = await readProcessMetrics(fixture, runtime.runtimePid)
  const sample: ScaleSample = {
    name: options.name,
    count: frameDurations.length,
    warmupRuns,
    measuredRuns,
    p50: percentile(frameDurations, 0.5),
    p95: percentile(frameDurations, 0.95),
    max: maximum(frameDurations),
    electronPid: processMetrics.electronPid,
    rendererPid: processMetrics.rendererPid,
    runtimePid: runtime.runtimePid,
    rendererRssMb: processMetrics.rendererRssMb,
    runtimeRssMb: processMetrics.runtimeRssMb,
    ptyCount: runtime.ptyCount,
    ptyPids: runtime.ptyPids,
    domNodes,
    statementCount: runtime.statementCount,
    longTaskCount: longTasks.length,
    longTaskP95: percentile(longTasks, 0.95),
    longTaskMax: maximum(longTasks),
    machine: machineInformation()
  }
  console.log(`[scale-baseline] ${JSON.stringify(sample)}`)
  return sample
}

async function collectBrowserFrames(
  fixture: MatouFixture,
  minimumFrameCount: number
): Promise<BrowserFrameSample> {
  return fixture.page.evaluate(async ({ frameCount }) => {
    const longTasks: number[] = []
    const observer = typeof PerformanceObserver === 'undefined'
      ? undefined
      : new PerformanceObserver((entries) => {
          for (const entry of entries.getEntries()) longTasks.push(entry.duration)
        })
    try {
      observer?.observe({ type: 'longtask', buffered: false })
    } catch {
      observer?.disconnect()
    }
    const timestamps: number[] = []
    await new Promise<void>((resolve) => {
      const frame = (timestamp: number) => {
        timestamps.push(timestamp)
        if (timestamps.length >= frameCount + 1) resolve()
        else requestAnimationFrame(frame)
      }
      requestAnimationFrame(frame)
    })
    observer?.takeRecords().forEach((entry) => longTasks.push(entry.duration))
    observer?.disconnect()
    return {
      frameDurations: timestamps.slice(1).map((timestamp, index) => timestamp - timestamps[index]!),
      longTasks,
      domNodes: document.getElementsByTagName('*').length
    }
  }, { frameCount: minimumFrameCount })
}

async function readRuntimeMetrics(
  fixture: MatouFixture,
  resetStatementCount: boolean
): Promise<RuntimeScaleMetrics> {
  return fixture.app.evaluate(async (_electron, reset) => {
    const read = (globalThis as typeof globalThis & {
      __matouE2eScaleMetrics?: (
        options: { resetStatementCount?: boolean }
      ) => Promise<RuntimeScaleMetrics>
    }).__matouE2eScaleMetrics
    if (!read) throw new Error('MATOU_E2E_SCALE Runtime metrics bridge is unavailable')
    return read({ resetStatementCount: reset })
  }, resetStatementCount)
}

async function readProcessMetrics(
  fixture: MatouFixture,
  runtimePid: number
): Promise<ProcessScaleMetrics> {
  return fixture.app.evaluate(({ app, BrowserWindow }, expectedRuntimePid) => {
    const mainWindow = BrowserWindow.getAllWindows().find((window) => !window.isDestroyed())
    if (!mainWindow) throw new Error('Scale benchmark main BrowserWindow is unavailable')
    const rendererPid = mainWindow.webContents.getOSProcessId()
    const metrics = app.getAppMetrics()
    const renderer = metrics.find(({ pid }) => pid === rendererPid)
    const runtime = metrics.find(({ pid }) => pid === expectedRuntimePid)
    if (!renderer?.memory || !runtime?.memory) {
      throw new Error(
        `Electron process metrics missing renderer=${rendererPid} or runtime=${expectedRuntimePid}`
      )
    }
    return {
      electronPid: process.pid,
      rendererPid,
      rendererRssMb: renderer.memory.workingSetSize / 1024,
      runtimeRssMb: runtime.memory.workingSetSize / 1024
    }
  }, runtimePid)
}

function machineInformation(): ScaleSample['machine'] {
  const processors = cpus()
  return {
    hostname: hostname(),
    platform: platform(),
    release: release(),
    cpuModel: processors[0]?.model ?? 'unknown',
    cpuCount: processors.length,
    totalMemoryMb: bytesToMb(totalmem()),
    freeMemoryMb: bytesToMb(freemem())
  }
}

function percentile(values: number[], quantile: number): number {
  if (values.length === 0) return 0
  const ordered = [...values].sort((left, right) => left - right)
  const index = Math.min(ordered.length - 1, Math.ceil(ordered.length * quantile) - 1)
  return round(ordered[index]!)
}

function maximum(values: number[]): number {
  return values.length === 0 ? 0 : round(Math.max(...values))
}

function bytesToMb(value: number): number {
  return round(value / (1024 * 1024))
}

function round(value: number): number {
  return Math.round(value * 100) / 100
}

function assertPositiveInteger(value: number, label: string): void {
  if (!Number.isSafeInteger(value) || value < 1) throw new Error(`${label} must be a positive integer`)
}

function assertNonNegativeInteger(value: number, label: string): void {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error(`${label} must be a non-negative integer`)
}
