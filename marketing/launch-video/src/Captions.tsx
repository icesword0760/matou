import { useMemo } from 'react'
import { splitCues } from './captions-split'
import type { CueData } from './manifest'

/** A cue is shown slightly before it is spoken and lingers a beat after, so it never flickers. */
const LEAD_IN_MS = 120
const LEAD_OUT_MS = 80

export const Captions = ({ cues, nowMs }: { cues: CueData[]; nowMs: number }) => {
  // The narration cues are whole sentence groups; `splitCues` breaks the long ones into lines that
  // never grow past two rows. See `captions-split.ts`.
  const lines = useMemo(() => splitCues(cues), [cues])
  const cue = lines.find((c) => nowMs >= c.startMs - LEAD_IN_MS && nowMs < c.endMs + LEAD_OUT_MS)
  if (!cue) return null
  return (
    <div
      style={{
        position: 'absolute',
        left: 0,
        right: 0,
        bottom: 64,
        display: 'flex',
        justifyContent: 'center',
        pointerEvents: 'none'
      }}
    >
      <div
        style={{
          fontFamily: '"PingFang SC", "Hiragino Sans GB", sans-serif',
          fontWeight: 600,
          fontSize: 40,
          lineHeight: 1.35,
          color: '#fff',
          padding: '10px 26px',
          borderRadius: 14,
          background: 'rgba(8,10,14,0.62)',
          textShadow: '0 2px 6px rgba(0,0,0,.8)',
          maxWidth: 1500,
          textAlign: 'center'
        }}
      >
        {cue.text}
      </div>
    </div>
  )
}
