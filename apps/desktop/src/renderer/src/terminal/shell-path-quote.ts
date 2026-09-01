const ORDINARY_PATH = /^[A-Za-z0-9_./:@%+=,-]+$/
const SPACE_ONLY_SPECIAL_PATH = /^[A-Za-z0-9_./:@%+=,-]+(?: +[A-Za-z0-9_./:@%+=,-]+)+$/

/**
 * Preserve the existing visible form for ordinary paths while making every
 * other filename one literal zsh argv. POSIX single quotes are the fallback;
 * an embedded single quote is represented by closing, escaping, and reopening
 * the quoted string.
 */
export function quoteDroppedPath(path: string): string {
  if (ORDINARY_PATH.test(path)) return path
  if (SPACE_ONLY_SPECIAL_PATH.test(path)) return `"${path}"`
  return `'${path.replaceAll("'", "'\\''")}'`
}
