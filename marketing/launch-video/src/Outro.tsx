import { AbsoluteFill, Img, spring, staticFile, useCurrentFrame, useVideoConfig } from 'remotion'

const SANS = '"PingFang SC", "Hiragino Sans GB", sans-serif'
const MONO = 'Menlo, monospace'
/** Each line lands 12 frames after the one above it. */
const STAGGER = 12

/** Main size is 56; the monospace URL is set smaller so it clears the QR block on the right. */
const LINES: { text: string; font: string; color: string; size: number }[] = [
  { text: '早期预览 · 仅 Apple Silicon', font: SANS, color: '#e6edf3', size: 56 },
  { text: '安装包未签名，首次打开方法见置顶评论', font: SANS, color: '#e6edf3', size: 56 },
  { text: '演示数据为构造', font: SANS, color: '#8b96a3', size: 56 },
  { text: 'github.com/icesword0760/matou · GPL-3.0', font: MONO, color: '#ffd166', size: 46 }
]

/** The closing card: the four disclaimers fade up one by one, then the QQ group + logo. */
export const Outro = () => {
  const frame = useCurrentFrame()
  const { fps } = useVideoConfig()
  // damping 200 = no overshoot; these should settle, not bounce.
  const enter = (index: number) => spring({ frame: frame - index * STAGGER, fps, config: { damping: 200 } })
  return (
    <AbsoluteFill style={{ background: 'radial-gradient(ellipse at 30% 45%, #1b2129 0%, #0b0f14 72%)' }}>
      <div style={{ position: 'absolute', left: 140, top: 300, display: 'flex', flexDirection: 'column', gap: 34 }}>
        {LINES.map((line, index) => {
          const appear = enter(index)
          return (
            <div
              key={line.text}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 22,
                opacity: appear,
                transform: `translateY(${(1 - appear) * 26}px)`,
                fontFamily: line.font,
                fontWeight: 600,
                fontSize: line.size,
                lineHeight: 1.2,
                color: line.color
              }}
            >
              <span style={{ width: 12, height: 12, borderRadius: 6, background: '#3d7dff', flexShrink: 0 }} />
              {line.text}
            </div>
          )
        })}
      </div>
      <div
        style={{
          position: 'absolute',
          right: 96,
          // High enough that even a two-line caption clears the QR card.
          bottom: 215,
          display: 'flex',
          alignItems: 'center',
          gap: 30,
          opacity: enter(LINES.length),
          transform: `translateY(${(1 - enter(LINES.length)) * 26}px)`
        }}
      >
        <Img src={staticFile('logo.png')} style={{ width: 132, height: 132, borderRadius: 26 }} />
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 12 }}>
          <Img
            src={staticFile('qq-group.png')}
            style={{ width: 232, borderRadius: 12, background: '#fff', padding: 8 }}
          />
          <span style={{ fontFamily: SANS, fontWeight: 600, fontSize: 28, color: '#8b96a3' }}>交流群</span>
        </div>
      </div>
    </AbsoluteFill>
  )
}
