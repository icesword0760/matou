import type {
  EgressCheckWire,
  EgressExit,
  EgressFamily,
  EgressFamilyWire,
  EgressStatusWire
} from '@matou/contracts'

export type ChipTone = 'neutral' | 'ok' | 'warn' | 'danger' | 'danger-outline'

export interface ChipCopy {
  readonly label: string
  readonly tone: ChipTone
}

export interface CheckCopy {
  readonly title: string
  readonly detail?: string
}

const zhDisplayNames =
  typeof Intl !== 'undefined' ? new Intl.DisplayNames(['zh-CN'], { type: 'region' }) : undefined
const enDisplayNames =
  typeof Intl !== 'undefined' ? new Intl.DisplayNames(['en'], { type: 'region' }) : undefined

/** Convert an ISO 3166-1 alpha-2 region code into its Unicode flag emoji. */
export function countryFlag(code: string | undefined): string {
  if (!code || !/^[A-Za-z]{2}$/.test(code)) return ''
  const upper = code.toUpperCase()
  return String.fromCodePoint(
    0x1f1e6 + upper.charCodeAt(0) - 65,
    0x1f1e6 + upper.charCodeAt(1) - 65
  )
}

/** Resolve localized region display name via Intl.DisplayNames. */
export function countryName(code: string | undefined, zh: boolean): string {
  if (!code) return zh ? '未知地区' : 'Unknown'
  const upper = code.toUpperCase()
  if (!/^[A-Z]{2}$/.test(upper)) return code
  try {
    const resolver = zh ? zhDisplayNames : enDisplayNames
    return resolver?.of(upper) ?? upper
  } catch {
    return upper
  }
}

/** Combine flag emoji and localized region name, e.g. `🇺🇸 美国`. */
export function countryLabel(code: string | undefined, zh: boolean): string {
  const flag = countryFlag(code)
  const name = countryName(code, zh)
  return flag ? `${flag} ${name}` : name
}

/** Count warnings relevant to a provider family (family checks + global checks). */
export function familyWarnCount(
  checks: readonly EgressCheckWire[],
  family: EgressFamily
): number {
  return checks.filter(
    (check) => (check.family === undefined || check.family === family) && check.result === 'warn'
  ).length
}

/** Compute the exact HUD chip label and visual tone for a provider family. */
export function chipCopy(
  statusState: EgressStatusWire['state'],
  family: EgressFamily,
  familyState: EgressFamilyWire | undefined,
  checks: readonly EgressCheckWire[],
  checkedAt: number | undefined,
  zh: boolean
): ChipCopy {
  if (statusState === 'checking' || checkedAt === undefined || familyState === undefined) {
    return {
      label: zh ? '出口检查中…' : 'Checking exit…',
      tone: 'neutral'
    }
  }
  if (familyState.overridden) {
    return {
      label: zh ? '出口异常 · 已手动继续' : 'Exit problem · Continued manually',
      tone: 'danger-outline'
    }
  }
  if (familyState.state === 'abnormal' && familyState.frozenSessionIds.length > 0) {
    const pausedCount = familyState.frozenSessionIds.length
    return {
      label: zh
        ? `出口异常 · 已暂停 ${pausedCount} 个会话`
        : `Exit problem · ${pausedCount} session(s) paused`,
      tone: 'danger'
    }
  }
  if (familyState.state === 'unknown') {
    return {
      label: zh ? '无法确认出口' : 'Exit unconfirmed',
      tone: 'warn'
    }
  }
  const region = countryLabel(familyState.exit?.country, zh)
  const warns = familyWarnCount(checks, family)
  if (warns > 0) {
    return {
      label: zh ? `${region} · ${warns} 项提醒` : `${region} · ${warns} warning(s)`,
      tone: 'warn'
    }
  }
  return {
    label: zh ? `${region} · 出口正常` : `${region} · Exit OK`,
    tone: 'ok'
  }
}

function factString(check: EgressCheckWire, key: string): string {
  const value = check.facts?.[key]
  return typeof value === 'string' ? value : ''
}

/** Produce localized title and state detail for a check item. */
export function checkCopy(
  check: EgressCheckWire,
  zh: boolean,
  familyExit?: EgressExit
): CheckCopy {
  const title = checkTitle(check.id, zh)

  if (check.result === 'unknown') {
    return {
      title,
      detail: zh ? '无法确认（检测超时）' : "Couldn't confirm (check timed out)"
    }
  }

  if (check.result === 'pass') {
    if (check.id === 'mainland') {
      const ip = familyExit?.ip ?? factString(check, 'ip')
      const countryCode = familyExit?.country ?? factString(check, 'country')
      if (ip || countryCode) {
        const region = countryName(countryCode, zh)
        return {
          title,
          detail: ip ? `${region} · ${ip}` : region
        }
      }
    }
    return { title }
  }

  switch (check.id) {
    case 'mainland': {
      const host = factString(check, 'host')
      return {
        title,
        detail: zh
          ? `${host} 的出口在中国大陆。检查代理是否开启；规则模式下确认该域名走代理。`
          : `${host} exits from mainland China. Check that your proxy is on and that this domain is routed through it.`
      }
    }
    case 'exit-rule': {
      const baseline = countryName(factString(check, 'baselineCountry'), zh)
      const current = countryName(factString(check, 'currentCountry'), zh)
      const ip = factString(check, 'currentIp')
      return {
        title,
        detail: zh
          ? `${baseline} → ${current}（${ip}）。确认节点；如果是预期变化，点「以当前出口为准」。`
          : `${baseline} → ${current} (${ip}). Check your node; if this change is expected, choose "Use current exit".`
      }
    }
    case 'login-api-match': {
      const loginIp = factString(check, 'loginIp')
      const apiIp = factString(check, 'apiIp')
      return {
        title,
        detail: zh
          ? `登录 ${loginIp}，API ${apiIp}。多半是分流规则把两个域名分到了不同节点。`
          : `Sign-in ${loginIp}, API ${apiIp}. A routing rule probably sends these domains through different nodes.`
      }
    }
    case 'agent-proxy-path':
      return {
        title,
        detail: zh
          ? '已开系统代理，但终端里的 agent 不读系统代理。请在 Clash 开启 TUN，或在 shell 配置里设置 HTTPS_PROXY。'
          : 'A system proxy is on, but terminal agents ignore it. Turn on TUN mode in Clash, or set HTTPS_PROXY in your shell profile.'
      }
    case 'browser-path': {
      const browserIp = factString(check, 'browserIp')
      const agentIp = factString(check, 'agentIp')
      return {
        title,
        detail: zh
          ? `浏览器 ${browserIp}，终端 ${agentIp}。授权登录和使用时 IP 不同。`
          : `Browser ${browserIp}, terminal ${agentIp}. You would sign in and work from different IPs.`
      }
    }
    case 'time-zone':
      return {
        title,
        detail: zh
          ? '会话会使用大陆时区。到 设置 → Claude Code → 时区，选「自动」。'
          : 'Sessions would use a mainland China time zone. In Settings → Claude Code → Time zone, choose Automatic.'
      }
    case 'ipv6':
      return {
        title,
        detail: zh
          ? '本机有公网 IPv6，部分 agent 可能经 IPv6 直连。可在 Clash 开启 IPv6 接管，或在系统里关闭 IPv6。'
          : 'This Mac has public IPv6, so some agents may connect directly over IPv6. Let Clash handle IPv6, or turn IPv6 off.'
      }
  }
}

function checkTitle(id: EgressCheckWire['id'], zh: boolean): string {
  switch (id) {
    case 'mainland':
      return zh ? '出口不在中国大陆' : 'Exit is outside mainland China'
    case 'exit-rule':
      return zh ? '出口符合出口规则' : 'Exit matches your rule'
    case 'login-api-match':
      return zh ? '登录与 API 同一出口' : 'Sign-in and API share one exit'
    case 'agent-proxy-path':
      return zh ? '终端 agent 走代理' : 'Terminal agents use the proxy'
    case 'browser-path':
      return zh ? '登录浏览器与终端同一出口' : 'Browser and terminal share one exit'
    case 'time-zone':
      return zh ? '没有暴露大陆时区' : 'No mainland China time zone'
    case 'ipv6':
      return zh ? '无 IPv6 绕过' : 'No IPv6 bypass'
  }
}

export function formatCheckedTime(timestamp: number): string {
  const date = new Date(timestamp)
  const hh = String(date.getHours()).padStart(2, '0')
  const mm = String(date.getMinutes()).padStart(2, '0')
  return `${hh}:${mm}`
}

export function ruleLabel(ruleMode: EgressStatusWire['ruleMode'], zh: boolean): string {
  if (ruleMode === 'country') {
    return zh ? '同一国家／地区（推荐）' : 'Same country/region (recommended)'
  }
  return zh ? '固定 IP' : 'Fixed IP'
}

/** Redact IPv4 addresses to `a.b.x.x` (and IPv6 to `a:b:x:x`) anywhere inside text. */
export function redactIpsInText(text: string): string {
  return text
    .replace(/\b(\d{1,3})\.(\d{1,3})\.\d{1,3}\.\d{1,3}\b/g, '$1.$2.x.x')
    .replace(/\b([0-9a-fA-F]{1,4}:[0-9a-fA-F]{1,4}):[0-9a-fA-F:]+\b/g, '$1:x:x')
}

export function maskIp(ip: string | undefined): string {
  if (!ip) return ''
  return redactIpsInText(ip)
}

/** Build a diagnostic report with all IPs masked to first two octets and zero session IDs. */
export function buildEgressReport(status: EgressStatusWire, zh: boolean): string {
  const lines: string[] = []
  const checkedAt =
    status.checkedAt !== undefined
      ? formatCheckedTime(status.checkedAt)
      : '—'
  lines.push(zh ? `网络检查 (${checkedAt})` : `Network check (${checkedAt})`)
  lines.push(
    `${zh ? '出口规则' : 'Exit rule'}: ${ruleLabel(status.ruleMode, zh)}`
  )
  lines.push(
    `${zh ? '出口保护' : 'Exit protection'}: ${status.protectionEnabled ? (zh ? '已开启' : 'On') : (zh ? '已关闭' : 'Off')}`
  )

  for (const family of ['anthropic', 'openai'] as const) {
    const entry = status.families.find((item) => item.family === family)
    if (!entry) continue
    const heading =
      family === 'anthropic'
        ? zh ? 'Claude（Anthropic）' : 'Claude (Anthropic)'
        : zh ? 'Codex（OpenAI）' : 'Codex (OpenAI)'
    const exitSummary = entry.exit
      ? ` — ${countryName(entry.exit.country, zh)} · ${maskIp(entry.exit.ip)}`
      : ''
    lines.push('')
    lines.push(`[${heading}] ${entry.state}${exitSummary}`)
    for (const check of status.checks.filter((c) => c.family === family)) {
      const copy = checkCopy(check, zh, entry.exit)
      const detail = copy.detail ? ` — ${redactIpsInText(copy.detail)}` : ''
      lines.push(`- [${check.result}] ${copy.title}${detail}`)
    }
  }

  const globalChecks = status.checks.filter((c) => c.family === undefined)
  if (globalChecks.length > 0) {
    lines.push('')
    lines.push(`[${zh ? '本机' : 'This Mac'}]`)
    for (const check of globalChecks) {
      const copy = checkCopy(check, zh)
      const detail = copy.detail ? ` — ${redactIpsInText(copy.detail)}` : ''
      lines.push(`- [${check.result}] ${copy.title}${detail}`)
    }
  }

  return redactIpsInText(lines.join('\n'))
}
