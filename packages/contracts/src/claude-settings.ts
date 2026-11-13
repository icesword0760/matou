export interface ClaudeSettingsSnapshot {
  /** `auto` replaces mainland China/HK/Macau time zones with a non-mainland UTC+8 zone; `system` inherits the launch environment; otherwise an IANA time zone. */
  timeZone: string
  systemTimeZone: string
  /** Effective time zone when `timeZone` is `auto` and `systemTimeZone` is a mainland China/HK/Macau zone; `undefined` otherwise. */
  autoTimeZone?: string
}
