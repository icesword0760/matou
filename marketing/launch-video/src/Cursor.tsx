/**
 * Synthetic pointer drawn on top of a recording (the Electron capture has no cursor).
 * `x` / `y` are output-frame pixels; it lives inside the zoom layer so it scales with the UI.
 */
export const Cursor = ({ x, y, pressed }: { x: number; y: number; pressed: boolean }) => (
  <svg style={{ position: 'absolute', left: x - 4, top: y - 2, pointerEvents: 'none', filter: 'drop-shadow(0 2px 3px rgba(0,0,0,.5))' }} width="34" height="40" viewBox="0 0 17 20">
    {pressed && <circle cx="3" cy="3" r="9" fill="rgba(80,160,255,0.35)" />}
    <path d="M1 1 L1 15.5 L4.6 12.2 L7.3 18.4 L9.8 17.3 L7.2 11.3 L12 11.3 Z" fill="#fff" stroke="#000" strokeWidth="1.2" strokeLinejoin="round" />
  </svg>
)
