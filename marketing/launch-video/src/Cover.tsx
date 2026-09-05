import { AbsoluteFill, Img, Sequence, staticFile } from 'remotion'
import { Intro } from './Intro'
import { HEIGHT, WIDTH } from './manifest'

/** Bilibili's cover slot. */
export const COVER_WIDTH = 1146
export const COVER_HEIGHT = 717

/** Share of the cover given to the "before" half. */
const LEFT_SHARE = 0.45
/** All twelve windows are up and the error box is flashing by here. */
const INTRO_FRAME = 130

const SANS = '"PingFang SC", "Hiragino Sans GB", sans-serif'

const leftWidth = COVER_WIDTH * LEFT_SHARE
// The 1920x1080 intro is scaled to the panel's height and centred, which lands the error box
// across the panel - the one part of the chaos that still reads at this size.
const introScale = COVER_HEIGHT / HEIGHT
const introOffset = (leftWidth - WIDTH * introScale) / 2

/**
 * The 1146x717 still: terminal chaos crossed out on the left, the real workspace on the right,
 * one line of promise across the middle. Everything here has to survive being 286px wide.
 */
export const Cover = () => (
  <AbsoluteFill style={{ background: '#05070a', overflow: 'hidden' }}>
    <div style={{ position: 'absolute', left: 0, top: 0, width: leftWidth, height: COVER_HEIGHT, overflow: 'hidden' }}>
      <div
        style={{
          position: 'absolute',
          left: introOffset,
          top: 0,
          width: WIDTH,
          height: HEIGHT,
          transformOrigin: '0 0',
          transform: `scale(${introScale})`
        }}
      >
        {/* A negative `from` is how you hold a component at a frame in a still: `<Freeze>` only
            rewrites the timeline context, which the still renderer does not read. */}
        <Sequence from={-INTRO_FRAME} layout="none">
          <Intro />
        </Sequence>
      </div>
      {/* Scrim so the chaos reads as the rejected half without going black. */}
      <AbsoluteFill style={{ background: 'linear-gradient(90deg, rgba(5,7,10,.12) 0%, rgba(5,7,10,.62) 100%)' }} />
      <svg
        width={leftWidth}
        height={COVER_HEIGHT}
        viewBox={`0 0 ${leftWidth} ${COVER_HEIGHT}`}
        style={{ position: 'absolute', left: 0, top: 0 }}
      >
        <g
          stroke="#ff3b30"
          strokeWidth={22}
          strokeLinecap="round"
          opacity={0.9}
          style={{ filter: 'drop-shadow(0 0 16px rgba(255,59,48,.5))' }}
        >
          <line x1={70} y1={96} x2={leftWidth - 70} y2={COVER_HEIGHT - 96} />
          <line x1={leftWidth - 70} y1={96} x2={70} y2={COVER_HEIGHT - 96} />
        </g>
      </svg>
    </div>

    <div
      style={{
        position: 'absolute',
        left: leftWidth,
        top: 0,
        width: COVER_WIDTH - leftWidth,
        height: COVER_HEIGHT,
        padding: 14,
        boxSizing: 'border-box'
      }}
    >
      <Img
        src={staticFile('workspace-demo.png')}
        style={{
          width: '100%',
          height: '100%',
          objectFit: 'cover',
          objectPosition: 'left center',
          borderRadius: 16,
          border: '1px solid rgba(120,140,170,.35)',
          boxShadow: '0 24px 60px rgba(0,0,0,.65)'
        }}
      />
    </div>

    {/* The title band spans both halves, so the promise - not the split - is what you read first. */}
    <div
      style={{
        position: 'absolute',
        left: 0,
        right: 0,
        top: 236,
        padding: '34px 0 40px',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        gap: 14,
        background:
          'linear-gradient(180deg, rgba(4,6,10,0) 0%, rgba(4,6,10,.8) 22%, rgba(4,6,10,.8) 78%, rgba(4,6,10,0) 100%)'
      }}
    >
      <div
        style={{
          fontFamily: SANS,
          fontWeight: 800,
          fontSize: 78,
          lineHeight: 1.1,
          color: '#fff',
          whiteSpace: 'nowrap',
          WebkitTextStrokeWidth: 6,
          WebkitTextStrokeColor: '#000',
          paintOrder: 'stroke fill',
          textShadow: '0 8px 26px rgba(0,0,0,.85)'
        }}
      >
        10 个 Claude Code 同时跑
      </div>
      <div
        style={{
          fontFamily: SANS,
          fontWeight: 700,
          fontSize: 40,
          letterSpacing: 3,
          color: '#ffd166',
          WebkitTextStrokeWidth: 4,
          WebkitTextStrokeColor: '#000',
          paintOrder: 'stroke fill',
          textShadow: '0 3px 12px rgba(0,0,0,.9)'
        }}
      >
        不迷路 · 开源 · Mac
      </div>
    </div>
  </AbsoluteFill>
)
