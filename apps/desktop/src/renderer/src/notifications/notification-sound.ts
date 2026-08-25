interface NotificationAudioContext {
  currentTime: number
  destination: unknown
  createOscillator(): {
    type: string
    connect(destination: unknown): void
    frequency: { setValueAtTime(value: number, time: number): void }
    start(time: number): void
    stop(time: number): void
  }
  createGain(): {
    connect(destination: unknown): void
    gain: {
      setValueAtTime(value: number, time: number): void
      exponentialRampToValueAtTime(value: number, time: number): void
    }
  }
}

let audioContext: NotificationAudioContext | undefined

export function playNotificationSound(
  contextFactory: () => NotificationAudioContext = browserAudioContext
): void {
  try {
    const context = contextFactory()
    const oscillator = context.createOscillator()
    const gain = context.createGain()
    oscillator.connect(gain)
    gain.connect(context.destination)
    oscillator.type = 'sine'
    oscillator.frequency.setValueAtTime(880, context.currentTime)
    oscillator.frequency.setValueAtTime(1047, context.currentTime + 0.1)
    gain.gain.setValueAtTime(0.3, context.currentTime)
    gain.gain.exponentialRampToValueAtTime(0.001, context.currentTime + 0.3)
    oscillator.start(context.currentTime)
    oscillator.stop(context.currentTime + 0.3)
  } catch (error) {
    console.warn('[NotificationSound] 播放失败:', error)
  }
}

function browserAudioContext(): NotificationAudioContext {
  if (audioContext) return audioContext
  const AudioContextClass = window.AudioContext ??
    (window as typeof window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
  if (!AudioContextClass) throw new Error('WebAudio is unavailable')
  audioContext = new AudioContextClass() as unknown as NotificationAudioContext
  return audioContext
}
