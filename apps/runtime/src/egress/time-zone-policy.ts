import { getAgentManifest, type AgentProfileId } from '@matou/domain'

const MAINLAND_TIME_ZONES = new Set([
  'Asia/Shanghai',
  'Asia/Chongqing',
  'Asia/Chungking',
  'Asia/Harbin',
  'Asia/Urumqi',
  'Asia/Kashgar',
  'PRC',
  'Asia/Hong_Kong',
  'Hongkong',
  'Asia/Macau',
  'Asia/Macao'
])

const EXIT_UTC8_ZONES: Record<string, string> = {
  TW: 'Asia/Taipei',
  MY: 'Asia/Kuala_Lumpur',
  PH: 'Asia/Manila'
}

export function isMainlandTimeZone(zone: string): boolean {
  return MAINLAND_TIME_ZONES.has(zone)
}

export function nonMainlandUtc8Zone(exitCountry?: string): string {
  const code = exitCountry?.toUpperCase()
  return (code && EXIT_UTC8_ZONES[code]) || 'Asia/Singapore'
}

export function agentTimeZone(input: {
  profile: AgentProfileId
  setting: string
  systemTimeZone: string
  exitCountry?: string
}): string | undefined {
  const effectiveSetting = getAgentManifest(input.profile)?.capabilities.claudeTimeZone
    ? input.setting
    : 'auto'
  if (effectiveSetting === 'system') return undefined
  if (effectiveSetting === 'auto') {
    return isMainlandTimeZone(input.systemTimeZone) ? nonMainlandUtc8Zone(input.exitCountry) : undefined
  }
  return effectiveSetting
}
