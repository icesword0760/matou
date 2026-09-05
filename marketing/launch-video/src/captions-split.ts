import type { CueData } from './manifest'

/**
 * Longest caption line we let through. The narration cues are whole sentence groups - several of
 * them run past eighty characters - and at the caption's 40px type that wraps to four or five
 * lines, which eats the bottom third of the frame and covers the card strip the shot is about.
 * Twenty-eight full-width characters is what fits the 1500px caption box in two lines at most.
 */
export const MAX_CHARS = 28

/** Sentence-internal punctuation we are willing to break a cue at, in the narration's own style. */
const BOUNDARIES = new Set(['；', '。', '，', '：'])

/**
 * Closers that belong to the clause that just ended. The narration quotes what you say to the AI
 * ("...给我结论。"), and cutting on that 。would otherwise push the closing quote onto the next
 * caption, where it reads as an opening one.
 */
const CLOSERS = new Set(['”', '’', '」', '』', '）', '】', '》'])
/** Straight quotes open and close with the same character, so they need counting, not a lookup. */
const SYMMETRIC = new Set(['"', "'"])

const closesClauseAt = (text: string, index: number): boolean => {
  const mark = text[index]
  if (mark === undefined) return false
  if (CLOSERS.has(mark)) return true
  if (!SYMMETRIC.has(mark)) return false
  let seen = 0
  for (let i = 0; i < index; i += 1) if (text[i] === mark) seen += 1
  // Odd means this one closes a quote that is already open; even means it opens a new one, and
  // an opening quote belongs to the caption whose sentence it starts.
  return seen % 2 === 1
}

/**
 * Splits one cue's text at the boundary closest to its middle, keeping the punctuation at the end
 * of the left piece, and recurses until every piece fits `MAX_CHARS` or has no boundary left to
 * cut at (a long clause with no punctuation stays whole rather than being chopped mid-word).
 */
export const splitText = (text: string): string[] => {
  if (text.length <= MAX_CHARS) return [text]
  const middle = text.length / 2
  let best = -1
  // Never cut after the final character: that would only produce an empty right-hand piece.
  for (let index = 0; index < text.length - 1; index += 1) {
    if (!BOUNDARIES.has(text[index]!)) continue
    if (best < 0 || Math.abs(index + 1 - middle) < Math.abs(best + 1 - middle)) best = index
  }
  if (best < 0) return [text]
  let cut = best + 1
  while (cut < text.length - 1 && closesClauseAt(text, cut)) cut += 1
  return [...splitText(text.slice(0, cut)), ...splitText(text.slice(cut))]
}

/**
 * One cue as the caption pieces it is shown in, its time shared out in proportion to how much of
 * the text each piece carries - speech inside a cue runs at a near-constant rate, so character
 * count is a good enough clock. The last piece always ends exactly on the cue's own end, so
 * rounding can never open a gap before the next cue.
 */
export const splitCue = (cue: CueData): CueData[] => {
  const pieces = splitText(cue.text).map((piece) => piece.trim()).filter((piece) => piece.length > 0)
  if (pieces.length <= 1) return [cue]
  const total = pieces.reduce((sum, piece) => sum + piece.length, 0)
  const span = cue.endMs - cue.startMs
  let at = cue.startMs
  return pieces.map((text, index) => {
    const startMs = at
    const endMs = index === pieces.length - 1 ? cue.endMs : startMs + Math.round((span * text.length) / total)
    at = endMs
    return { startMs, endMs, text }
  })
}

export const splitCues = (cues: CueData[]): CueData[] => cues.flatMap(splitCue)
