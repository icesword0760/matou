import { useEffect, useRef } from 'react'

type SurfaceState = {
  nodes: number[]
  velocities: number[]
}

const BUBBLES = Array.from({ length: 11 }, (_, index) => ({
  x: ((index * 71) % 91 + 4) / 100,
  radius: 1 + (index % 3) * .72,
  speed: .68 + (index % 4) * .11,
  offset: ((index * 41) % 100) / 100
}))

const clamp = (value: number, minimum: number, maximum: number) =>
  Math.max(minimum, Math.min(maximum, value))

const smooth = (value: number) => value * value * (3 - 2 * value)
const RECOVERY_WATER_CYCLE_MS = 7_200

export function recoveryWaterTimeline(elapsed: number, reducedMotion: boolean) {
  if (reducedMotion) return { rise: .5, alpha: 1 }
  const cycleElapsed = Math.max(0, elapsed) % RECOVERY_WATER_CYCLE_MS
  const rise = .94 * smooth(1 - Math.exp(-cycleElapsed / 2_400))
  return {
    rise,
    alpha: clamp(cycleElapsed / 288, 0, 1)
  }
}

export function SessionRecoveryWater({ sessionTitle }: { sessionTitle: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    if (typeof CanvasRenderingContext2D === 'undefined') return

    const reducedMotion = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
    const surfaceState: SurfaceState = { nodes: [], velocities: [] }
    const animationOrigin = performance.now()
    let previousFrame = animationOrigin
    let cssWidth = 0
    let cssHeight = 0
    let slosh = 0
    let turbulence = 0
    let bulkPhase = 0
    let bulkSignal = 0
    let innerSignal = 0
    let counterSignal = 0
    let cycleIndex = -1
    let frame = 0
    let context: CanvasRenderingContext2D | null = null

    const resetSurface = () => {
      const count = Math.max(48, Math.min(76, Math.round(cssWidth / 7)))
      surfaceState.nodes = Array.from({ length: count }, (_, index) =>
        Math.sin((index / (count - 1)) * Math.PI) * 1.2)
      surfaceState.velocities = Array.from({ length: count }, (_, index) =>
        Math.cos((index / (count - 1)) * Math.PI) * .18)
    }

    const surfaceOffset = (x: number) => {
      const { nodes } = surfaceState
      const position = clamp((x / Math.max(cssWidth, 1)) * (nodes.length - 1), 0, nodes.length - 1)
      const left = Math.floor(position)
      const right = Math.min(nodes.length - 1, left + 1)
      const mix = position - left
      const displacement = (nodes[left] ?? 0) * (1 - mix) + (nodes[right] ?? 0) * mix
      const edgeDistance = Math.min(x, cssWidth - x)
      const meniscus = -3.1 * Math.exp(-Math.max(0, edgeDistance) / 11)
      const normalized = x / Math.max(cssWidth, 1)
      const tilt = slosh * (normalized - .5) * 2
      const delayedBody = Math.sin(normalized * Math.PI) * innerSignal * turbulence * 16
      const counterWave = Math.sin(normalized * Math.PI * 2) * counterSignal * turbulence * 8.2
      const shoulder = Math.sin(normalized * Math.PI * 3) * Math.sin(bulkPhase * 1.47 - 1.55) * turbulence * 4.4
      return displacement + meniscus + tilt + delayedBody + counterWave + shoulder
    }

    const traceSurface = (base: number) => {
      if (!context) return
      context.beginPath()
      context.moveTo(0, clamp(base + surfaceOffset(0), 2, cssHeight - 2))
      for (let x = 3; x <= cssWidth + 3; x += 3) {
        context.lineTo(x, clamp(base + surfaceOffset(x), 2, cssHeight - 2))
      }
    }

    const waterPath = (base: number) => {
      if (!context) return
      traceSurface(base)
      context.lineTo(cssWidth, cssHeight + 2)
      context.lineTo(0, cssHeight + 2)
      context.closePath()
    }

    const stepSurface = (frameScale: number, fillProgress: number, energy: number) => {
      if (reducedMotion) return
      const { nodes, velocities } = surfaceState
      const nextVelocity = velocities.slice()
      const dampingStrength = .022 + fillProgress * .07
      const limit = 2.5 + energy * 18
      for (let index = 1; index < nodes.length - 1; index += 1) {
        const position = nodes[index] ?? 0
        const velocity = velocities[index] ?? 0
        const spring = -position * .026
        const damping = -velocity * dampingStrength
        const coupling = ((nodes[index - 1] ?? 0) + (nodes[index + 1] ?? 0) - position * 2) * .2
        nextVelocity[index] = (nextVelocity[index] ?? 0) + (spring + damping + coupling) * frameScale
      }
      for (let index = 1; index < nodes.length - 1; index += 1) {
        const velocity = nextVelocity[index] ?? 0
        velocities[index] = velocity
        nodes[index] = clamp((nodes[index] ?? 0) + velocity * frameScale, -limit, limit)
      }
      const drive = (Math.cos(bulkPhase) + Math.cos(bulkPhase * 1.74 + .5) * .24)
        * (.03 + energy * .15) * frameScale
      velocities[1] = (velocities[1] ?? 0) + drive
      velocities[nodes.length - 2] = (velocities[nodes.length - 2] ?? 0) - drive
      for (let index = 0; index < velocities.length; index += 1) {
        velocities[index] = clamp(velocities[index] ?? 0, -1.8, 1.8)
      }
      nodes[0] = (nodes[1] ?? 0) * .72
      nodes[nodes.length - 1] = (nodes[nodes.length - 2] ?? 0) * .72
    }

    const drawFluid = () => {
      frame = 0
      if (document.hidden) return

      const bounds = canvas.getBoundingClientRect()
      const nextWidth = Math.max(0, Math.round(bounds.width))
      const nextHeight = Math.max(0, Math.round(bounds.height))
      if (nextWidth === 0 || nextHeight === 0) return

      if (!context) context = canvas.getContext('2d')
      if (!context) return

      const dpr = Math.min(window.devicePixelRatio || 1, 2)
      if (nextWidth !== cssWidth || nextHeight !== cssHeight || canvas.width !== Math.round(nextWidth * dpr)) {
        cssWidth = nextWidth
        cssHeight = nextHeight
        canvas.width = Math.round(cssWidth * dpr)
        canvas.height = Math.round(cssHeight * dpr)
        context.setTransform(dpr, 0, 0, dpr, 0, 0)
        resetSurface()
      }
      context.clearRect(0, 0, cssWidth, cssHeight)

      const now = performance.now()
      const elapsed = now - animationOrigin
      const nextCycleIndex = reducedMotion ? 0 : Math.floor(elapsed / RECOVERY_WATER_CYCLE_MS)
      if (nextCycleIndex !== cycleIndex) {
        cycleIndex = nextCycleIndex
        resetSurface()
      }
      const cycleElapsed = reducedMotion ? elapsed : elapsed % RECOVERY_WATER_CYCLE_MS
      const frameScale = clamp((now - previousFrame) / 16.667, .35, 1.5)
      previousFrame = now
      const timeline = recoveryWaterTimeline(elapsed, reducedMotion)
      const rise = timeline.rise
      const surface = cssHeight * (.85 - rise * .74)
      const alpha = timeline.alpha
      const energy = reducedMotion ? .08 : Math.pow(1 - rise, 1.45)
      const introIntensity = reducedMotion ? 1 : 1 + (1 - smooth(clamp(cycleElapsed / 1700, 0, 1))) * .65
      bulkPhase = cycleElapsed * .00272 + 2.1 * (1 - Math.exp(-cycleElapsed / 1050)) - 1.08
      bulkSignal = reducedMotion ? 0 : (
        Math.sin(bulkPhase) + Math.sin(bulkPhase * .53 + .75) * .2 + Math.sin(bulkPhase * 1.83 + 1.4) * .12
      ) / 1.24
      innerSignal = reducedMotion ? 0 : (
        Math.sin(bulkPhase - .82) + Math.sin(bulkPhase * 1.66 + .35) * .24
      ) / 1.18
      counterSignal = reducedMotion ? 0 : Math.sin(bulkPhase * 1.52 + 1.72)
      slosh = bulkSignal * (2.4 + energy * 31.5) * introIntensity
      turbulence = reducedMotion ? 0 : Math.pow(1 - rise, .96) * introIntensity
      stepSurface(frameScale, rise, Math.min(1.5, energy * introIntensity))

      context.save()
      context.globalAlpha = alpha
      waterPath(surface)
      const waterGradient = context.createLinearGradient(0, surface - 5, 0, cssHeight)
      waterGradient.addColorStop(0, 'rgba(91,194,239,.36)')
      waterGradient.addColorStop(.08, 'rgba(58,164,226,.32)')
      waterGradient.addColorStop(.54, 'rgba(31,116,197,.39)')
      waterGradient.addColorStop(1, 'rgba(17,72,151,.52)')
      context.fillStyle = waterGradient
      context.fill()

      context.save()
      waterPath(surface)
      context.clip()
      const sheenCenter = clamp(.5 + innerSignal * turbulence * .22, .24, .76)
      const sheen = context.createLinearGradient(0, 0, cssWidth, 0)
      sheen.addColorStop(0, 'rgba(193,231,247,0)')
      sheen.addColorStop(clamp(sheenCenter - .16, .02, .72), 'rgba(193,231,247,0)')
      sheen.addColorStop(sheenCenter, `rgba(193,231,247,${.025 + turbulence * .035})`)
      sheen.addColorStop(clamp(sheenCenter + .2, .28, .98), 'rgba(193,231,247,0)')
      sheen.addColorStop(1, 'rgba(193,231,247,0)')
      context.fillStyle = sheen
      context.fillRect(0, surface - 20, cssWidth, cssHeight - surface + 20)

      for (let index = 0; index < 5; index += 1) {
        const anchor = cssWidth * ((index + 1) / 6)
        const drift = anchor + bulkSignal * (34 - index * 3)
          + innerSignal * (index % 2 === 0 ? 23 : -17) * turbulence
        const y = surface + 58 + index * 63
        context.strokeStyle = `rgba(184,229,248,${.022 + index * .006})`
        context.lineWidth = .8 + (index % 2) * .35
        context.beginPath()
        context.moveTo(drift - 75, y + 5)
        context.bezierCurveTo(drift - 28, y - 6, drift + 23, y + 9, drift + 76, y - 4)
        context.stroke()
      }

      for (const bubble of BUBBLES) {
        const progress = (bubble.offset + cycleElapsed * .000065 * bubble.speed) % 1
        const y = cssHeight + 9 - progress * Math.max(cssHeight - surface + 24, 1)
        if (y < surface + 14 || y > cssHeight + 5) continue
        const x = cssWidth * bubble.x + bulkSignal * 22
          + innerSignal * turbulence * (bubble.radius * 4.5)
          + Math.sin(progress * 7 + bubble.offset * 8) * 3
        context.strokeStyle = `rgba(215,243,252,${.15 + bubble.radius * .045})`
        context.lineWidth = .65
        context.beginPath()
        context.arc(x, y, bubble.radius, 0, Math.PI * 2)
        context.stroke()
      }
      context.restore()

      context.shadowColor = 'rgba(92,188,237,.36)'
      context.shadowBlur = 5
      context.strokeStyle = 'rgba(211,242,251,.78)'
      context.lineWidth = 1.05
      traceSurface(surface)
      context.stroke()
      context.shadowBlur = 0

      for (let x = 2; x < cssWidth - 2; x += 7) {
        const slope = clamp(Math.abs(surfaceOffset(x + 2) - surfaceOffset(x - 2)) / 4, 0, 1)
        if (slope < .08) continue
        context.strokeStyle = `rgba(239,252,255,${.08 + slope * .24})`
        context.lineWidth = .7
        context.beginPath()
        context.moveTo(x, surface + surfaceOffset(x) - .6)
        context.lineTo(x + 4, surface + surfaceOffset(x + 4) - .6)
        context.stroke()
      }
      context.restore()

      if (!reducedMotion) frame = window.requestAnimationFrame(drawFluid)
    }

    const start = () => {
      if (frame || document.hidden) return
      previousFrame = performance.now()
      frame = window.requestAnimationFrame(drawFluid)
    }
    const onVisibilityChange = () => {
      if (document.hidden) {
        if (frame) window.cancelAnimationFrame(frame)
        frame = 0
      } else {
        start()
      }
    }
    const resizeObserver = typeof ResizeObserver === 'undefined'
      ? undefined
      : new ResizeObserver(start)
    resizeObserver?.observe(canvas)
    document.addEventListener('visibilitychange', onVisibilityChange)
    start()

    return () => {
      if (frame) window.cancelAnimationFrame(frame)
      resizeObserver?.disconnect()
      document.removeEventListener('visibilitychange', onVisibilityChange)
    }
  }, [])

  return <div className="session-recovery-overlay session-recovery-water" data-testid="session-recovery-water"
    role="status" aria-label={`正在恢复终端：${sessionTitle}`}
    onPointerDown={(event) => event.stopPropagation()}>
    <span className="visually-hidden">正在恢复最近的终端内容与运行状态</span>
    <canvas ref={canvasRef} className="session-recovery-water__canvas" aria-hidden="true" />
  </div>
}
