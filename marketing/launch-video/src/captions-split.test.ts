import { describe, expect, it } from 'vitest'
import { MAX_CHARS, splitCue, splitCues, splitText } from './captions-split'

describe('splitText', () => {
  it('leaves a short line alone', () => {
    expect(splitText('结构分四层。')).toEqual(['结构分四层。'])
  })
  it('cuts at the boundary closest to the middle and keeps the punctuation on the left', () => {
    // Boundaries sit at index 8 and 21 of a 30-character line: 8 is the closer to the middle, and
    // the trailing 。at the end is never a cut point because it would leave an empty right piece.
    expect(splitText('点一下 Fork，从当前会话分出一个子会话，继承全部上下文。')).toEqual([
      '点一下 Fork，',
      '从当前会话分出一个子会话，继承全部上下文。'
    ])
  })
  it('recurses until every piece fits', () => {
    const long = '工作空间对应一个仓库；事项对应一件能交付的活；画布是事项里的阶段，比如方案探索、实现、回归；每张卡片是一个独立的 Claude Code，也可以是普通 Shell。'
    const pieces = splitText(long)
    expect(pieces.length).toBeGreaterThan(2)
    for (const piece of pieces) expect(piece.length).toBeLessThanOrEqual(MAX_CHARS)
    expect(pieces.join('')).toBe(long)
  })
  it('keeps a closing quote with the clause it closes', () => {
    const quoted = '在任何一张卡片里你可以直接说："看看左边那张卡片的测试跑到哪了。"它会自己去读隔壁的屏幕。'
    const pieces = splitText(quoted)
    // The closing quote rides with the quoted sentence; the opening one stays with the sentence
    // it opens rather than being left at the end of the previous caption.
    expect(pieces).toContain('"看看左边那张卡片的测试跑到哪了。"')
    expect(pieces).toContain('它会自己去读隔壁的屏幕。')
    expect(pieces.join('')).toBe(quoted)
  })
  it('keeps a long clause whole when it has no boundary to cut at', () => {
    const unbroken = 'a'.repeat(MAX_CHARS + 10)
    expect(splitText(unbroken)).toEqual([unbroken])
  })
})

describe('splitCue', () => {
  const cue = { startMs: 1000, endMs: 5000, text: '前半句在这里，后半句也在这里，第三句同样在这里，第四句收尾。' }
  it('shares the cue time out in proportion to character count', () => {
    const pieces = splitCue(cue)
    expect(pieces.length).toBeGreaterThan(1)
    expect(pieces[0]!.startMs).toBe(1000)
    expect(pieces[pieces.length - 1]!.endMs).toBe(5000)
    for (let i = 1; i < pieces.length; i += 1) expect(pieces[i]!.startMs).toBe(pieces[i - 1]!.endMs)
    const long = pieces.reduce((a, b) => (a.text.length >= b.text.length ? a : b))
    const short = pieces.reduce((a, b) => (a.text.length <= b.text.length ? a : b))
    expect(long.endMs - long.startMs).toBeGreaterThanOrEqual(short.endMs - short.startMs)
  })
  it('returns a short cue untouched', () => {
    const small = { startMs: 0, endMs: 900, text: '下期见。' }
    expect(splitCue(small)).toEqual([small])
  })
})

describe('splitCues', () => {
  it('keeps the cues in order and never overlaps them', () => {
    const out = splitCues([
      { startMs: 0, endMs: 4000, text: '第一句比较长，需要被切成两半，这样字幕才不会超过两行。' },
      { startMs: 4000, endMs: 5000, text: '第二句很短。' }
    ])
    for (let i = 1; i < out.length; i += 1) expect(out[i]!.startMs).toBeGreaterThanOrEqual(out[i - 1]!.endMs)
  })
})
