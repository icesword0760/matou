import type { SubscriptionProvider } from '@matou/contracts'

/** Checked again in Electron main immediately before opening the browser. */
export function isSubscriptionAuthorizationUrl(provider: SubscriptionProvider, value: string): boolean {
  try {
    const url = new URL(value)
    if (url.protocol !== 'https:' || url.username || url.password || url.hash || url.port) return false
    const allowed = provider === 'codex'
      ? url.hostname === 'auth.openai.com' && ['/oauth/authorize', '/api/accounts/authorize'].includes(url.pathname)
      : provider === 'antigravity' && url.hostname === 'accounts.google.com' && ['/o/oauth2/v2/auth', '/o/oauth2/auth'].includes(url.pathname)
    if (!allowed) return false
    const redirect = new URL(url.searchParams.get('redirect_uri') ?? '')
    return redirect.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(redirect.hostname) &&
      redirect.port === (provider === 'codex' ? '1455' : '51121') &&
      redirect.pathname === (provider === 'codex' ? '/auth/callback' : '/oauth-callback') && !redirect.username && !redirect.password && !redirect.search && !redirect.hash
  } catch { return false }
}
