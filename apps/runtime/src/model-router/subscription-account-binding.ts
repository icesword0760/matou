/** A revoked connection must never silently adopt another account. */
export class SubscriptionAccountBindingError extends Error {
  constructor() { super(process.env.MATOU_LOCALE === 'en' ? 'Subscription account changed. Restart this session to use the account.' : '订阅账号已变更，请重新启动会话以使用账号。') }
}
