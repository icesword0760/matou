import { readCredentialJson } from './credential-file'

/** One OAuth token must look like one client.
 *
 * Antigravity runtime requests (inference, quota and model probes, settings
 * reads) are all issued with the same Hub user agent. Matou's own product name
 * never appears on provider traffic. The router derives its default value from
 * the Hub manifest, so the fallback release is mirrored here and written onto
 * the credential: a later manifest fetch can no longer make inference drift
 * away from the probes that share this token.
 *
 * The platform segment mirrors the pinned router default (`os_type=darwin;
 * arch=arm64`); changing it here without changing the router would split the
 * identity again. Keep this string in sync with the router release when the
 * verified Hub build changes.
 */
export const ANTIGRAVITY_CLIENT_USER_AGENT =
  'antigravity/hub/2.29.0 (aidev_client; os_type=darwin; arch=arm64; cl=1013060670)'

/** True when the credential file already carries exactly the shared agent.
 * Reading first keeps repeated launches from rewriting an identical value and
 * waking the engine watcher. */
export async function credentialCarriesAntigravityIdentity(
  dataRoot: string,
  file: { name: string; path?: unknown }
): Promise<boolean> {
  const credential = await readCredentialJson(dataRoot, file)
  return credential?.user_agent === ANTIGRAVITY_CLIENT_USER_AGENT
}
