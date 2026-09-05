import { AbsoluteFill, interpolate, spring, useCurrentFrame, useVideoConfig } from 'remotion'

const LINES = [
  '⏺ Bash(pnpm vitest run src/payments)',
  '  ⎿ Running…',
  '⏺ Read(src/payments/webhook.ts)',
  '> 给支付回调加幂等处理',
  '⏺ Update(prisma/schema.prisma)',
  '✻ Welcome to Claude Code!'
]

/** Deterministic pseudo-random in [0,1) - render must be identical on every pass. */
const seeded = (i: number) => ((i * 9301 + 49297) % 233280) / 233280

/** The cold open: twelve fake Claude Code windows pile up, then one of them is on fire. */
export const Intro = () => {
  const frame = useCurrentFrame()
  const { fps } = useVideoConfig()
  const windows = Array.from({ length: 12 }, (_, i) => {
    const appear = spring({ frame: frame - i * 9, fps, config: { damping: 14, stiffness: 120 } })
    const x = 120 + seeded(i) * 900
    const y = 60 + seeded(i + 40) * 420
    return (
      <div
        key={i}
        style={{
          position: 'absolute',
          left: x,
          top: y,
          width: 760,
          height: 460,
          opacity: appear,
          transform: `scale(${0.85 + 0.15 * appear}) rotate(${(seeded(i + 7) - 0.5) * 3}deg)`,
          background: '#111418',
          border: '1px solid #2a2f36',
          borderRadius: 10,
          boxShadow: '0 30px 60px rgba(0,0,0,.6)',
          fontFamily: 'Menlo, monospace',
          fontSize: 18,
          color: '#c7d0d9',
          padding: '44px 22px 22px',
          overflow: 'hidden'
        }}
      >
        <div style={{ position: 'absolute', top: 12, left: 14, display: 'flex', gap: 8 }}>
          {['#ff5f57', '#febc2e', '#28c840'].map((c) => (
            <span key={c} style={{ width: 12, height: 12, borderRadius: 6, background: c }} />
          ))}
        </div>
        <div style={{ position: 'absolute', top: 10, left: 0, right: 0, textAlign: 'center', color: '#7b8794', fontSize: 14 }}>
          claude — session {i + 1}
        </div>
        {LINES.map((l, k) => (
          <div key={k} style={{ opacity: 0.55 + seeded(i * 3 + k) * 0.45 }}>
            {l}
          </div>
        ))}
      </div>
    )
  })
  const errorAt = 12 * 9 + 20
  const flash = frame > errorAt ? interpolate((frame - errorAt) % 30, [0, 15, 30], [0.15, 0.55, 0.15]) : 0
  return (
    <AbsoluteFill style={{ background: 'radial-gradient(ellipse at 50% 40%, #1b2129 0%, #0b0f14 70%)' }}>
      {windows}
      {frame > errorAt && (
        <div
          style={{
            position: 'absolute',
            left: 560,
            top: 300,
            width: 800,
            padding: 28,
            background: `rgba(160,20,30,${flash})`,
            border: '2px solid #ff4d4f',
            borderRadius: 10,
            fontFamily: 'Menlo, monospace',
            fontSize: 22,
            color: '#ffd7d7'
          }}
        >
          Error: FAIL src/payments/webhook.duplicate.test.ts
          <br />
          exit code 1 · 30 min ago
        </div>
      )}
    </AbsoluteFill>
  )
}
