import type { RecorderEvent } from '../../../tests/e2e/launch-video/recorder'

export type ClickEvent = Extract<RecorderEvent, { type: 'click' }>

export const isClick = (event: RecorderEvent): event is ClickEvent => event.type === 'click'

/**
 * Clicks that must not drive the push-in, by the label `record.spec.ts` logs for them.
 *
 * Both are clicks on one window that are answered by a different window, so a shot anchored on
 * where the pointer went lands on footage it was never framed for:
 *
 * - `dag-node` opens the card and closes the DAG about 360ms later, so the hold would sit on the
 *   main window, zoomed into whatever happens to occupy the node's old position.
 * - `dag` is the 打开会话 DAG button in the main window's top right corner; the DAG window replaces
 *   the frame ~150ms later, already centred by the recorder, and a 1.35x push-in anchored on that
 *   button crops the graph. In ai-control the button is pressed 2.7s before the section ends, so
 *   the shot never gets to release and the whole payoff - three children fanning out - plays
 *   cropped.
 *
 * Filtering them out before `focusAt` also stops them extending a preceding shot through
 * `mergeGapMs`, and leaves `focusAt` a pure function of the clicks it is handed.
 */
const NO_ZOOM_LABELS: ReadonlySet<string> = new Set(['dag', 'dag-node'])

export const startsZoom = (event: ClickEvent): boolean =>
  event.label === undefined || !NO_ZOOM_LABELS.has(event.label)

/** Keeps a recorded coordinate inside the window the footage actually covers. */
export const clamp = (value: number, max: number): number => Math.min(max, Math.max(0, value))
