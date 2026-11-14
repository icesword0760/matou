import type { CheckpointWatermark } from '../checkpoints/checkpoint-manager'
import { TerminalScreenProjector } from '../control/terminal-screen-projector'
import type { SegmentJournal } from '../journal/segment-journal'

type Grid = { cols: number; rows: number }
interface ScreenCheckpoint extends Grid {
  terminalSequence: number
  screenEpoch: number
  snapshot: Uint8Array
}

export interface TerminalScreenRecoveryOptions {
  initialGrid?: Grid
  loadCheckpoint?: (watermark: CheckpointWatermark) => Promise<ScreenCheckpoint | undefined>
}

const RECOVERY_WRITE_BYTES = 256 * 1024

export async function recoverTerminalScreen(
  journal: SegmentJournal,
  grid: Grid,
  options: TerminalScreenRecoveryOptions | undefined,
  onError: (error: unknown) => void
) {
  const throughSequence = journal.lastSequence
  let checkpoint: ScreenCheckpoint | undefined
  if (throughSequence > 0) {
    try {
      const candidate = await options?.loadCheckpoint?.({
        terminalSequence: throughSequence,
        domainEventSequence: journal.domainEventSequenceAtOrBefore(throughSequence)
      })
      if (candidate && candidate.terminalSequence <= throughSequence) checkpoint = candidate
    } catch (error) {
      onError(error)
    }
  }
  let { cols, rows } = throughSequence > 0 ? checkpoint ?? options?.initialGrid ?? grid : grid
  let screenEpoch = checkpoint?.screenEpoch ?? 0
  let hasHistory = Boolean(checkpoint?.snapshot.byteLength)
  const screen = new TerminalScreenProjector(cols, rows)
  if (checkpoint) await screen.write(new TextDecoder().decode(checkpoint.snapshot))

  const fromSequence = (checkpoint?.terminalSequence ?? 0) + 1
  if (fromSequence <= throughSequence) {
    const decoder = new TextDecoder()
    let chunks: string[] = []
    let bytes = 0
    const flush = async () => {
      if (chunks.length > 0) await screen.write(chunks.join(''))
      chunks = []
      bytes = 0
    }
    try {
      for await (const frame of journal.iterateFrames({ fromSequence, throughSequence })) {
        if (frame.kind === 'output') {
          hasHistory ||= frame.data.byteLength > 0
          chunks.push(decoder.decode(frame.data, { stream: true }))
          bytes += frame.data.byteLength
          if (bytes >= RECOVERY_WRITE_BYTES) await flush()
        } else if (frame.kind === 'resize') {
          await flush()
          cols = frame.cols
          rows = frame.rows
          await screen.resize(cols, rows)
        } else if (frame.kind === 'reset') {
          await flush()
          decoder.decode()
          screenEpoch = frame.screenEpoch
          hasHistory = false
          await screen.reset()
        }
      }
    } catch (error) {
      // A damaged historical segment must not prevent a new process from starting.
      // Preserve any valid prefix we were able to project.
      onError(error)
    }
    chunks.push(decoder.decode())
    await flush()
  }

  const resized = cols !== grid.cols || rows !== grid.rows
  if (resized) await screen.resize(grid.cols, grid.rows)
  return { screen, screenEpoch, hasHistory, resized }
}

/** Roll the old viewport into normal scrollback before a new process redraws. */
export function terminalRunBoundary(rows: number): string {
  // Cancel an unfinished VT sequence, leave the alternate screen, normalize
  // origin/scroll margins, then make the old viewport scroll off intact.
  return '\x18\x1b[?1049l\x1b[!p' +
    '\x1b[?9;1000;1002;1003;1004;1005;1006;1015;1007;2004;2026l\x1b[?25h' +
    '\x1b[?6l\x1b[r\x1b[0m\x1b[9999;1H' + '\r\n'.repeat(rows) + '\x1b[H'
}
