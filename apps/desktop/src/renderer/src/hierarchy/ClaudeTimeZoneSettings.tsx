import { useEffect, useMemo, useState } from 'react'
import type { ClaudeSettingsSnapshot } from '@matou/contracts'
import { useLocale } from '../i18n/LocaleProvider'
import type { ProviderConfigClient } from './ModelSwitchSettings'
import './claude-time-zone-settings.css'

export function ClaudeTimeZoneSettings({ client }: { client: ProviderConfigClient | null }) {
  const zh = useLocale() === 'zh-CN'
  const t = (cn: string, en: string) => zh ? cn : en
  const [stored, setStored] = useState<ClaudeSettingsSnapshot>()
  const [draft, setDraft] = useState('system')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [saved, setSaved] = useState(false)
  const [retry, setRetry] = useState(0)

  useEffect(() => {
    let active = true
    setStored(undefined); setError(''); setSaved(false)
    if (!client) { setError(t('设置服务未连接，请重试。', 'Settings service is disconnected. Please retry.')); return }
    void client.request('claude-settings.get', {}).then(result => {
      if (!active) return
      const snapshot = result as ClaudeSettingsSnapshot
      setStored(snapshot); setDraft(snapshot.timeZone)
    }).catch(() => {
      if (active) setError(t('无法读取时区设置，请重试。', 'Could not load the time zone. Please retry.'))
    })
    return () => { active = false }
  }, [client, retry, zh])

  const zones = useMemo(() => {
    const common = ['UTC', 'Asia/Shanghai', 'Asia/Tokyo', 'Asia/Singapore', 'America/Los_Angeles', 'America/New_York', 'Europe/London', 'Europe/Paris']
    const intl = Intl as typeof Intl & { supportedValuesOf?: (key: 'timeZone') => string[] }
    const all = intl.supportedValuesOf?.('timeZone') ?? []
    return [...new Set([...common, ...(stored && stored.timeZone !== 'system' ? [stored.timeZone] : []), ...all])]
  }, [stored])
  const effective = draft === 'system' ? stored?.systemTimeZone : draft
  const preview = effective ? new Intl.DateTimeFormat(zh ? 'zh-CN' : 'en-US', {
    timeZone: effective, month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false
  }).format(new Date()) : ''

  const save = async () => {
    if (!client || !stored || saving) return
    setSaving(true); setError(''); setSaved(false)
    try {
      const snapshot = await client.request('claude-settings.set', { timeZone: draft }) as ClaudeSettingsSnapshot
      setStored(snapshot); setDraft(snapshot.timeZone); setSaved(true)
    } catch {
      setError(t('保存未确认，请重试。', 'Save was not confirmed. Please retry.'))
    } finally { setSaving(false) }
  }

  return <div className="claude-time-zone-settings" aria-busy={!stored && !error || saving}>
    <label htmlFor="claude-time-zone">{t('时区', 'Time zone')}</label>
    <p id="claude-time-zone-help">{t('用于 Claude Code 的额度重置时间等显示。', 'Used for Claude Code displays such as usage limit reset times.')}</p>
    <select id="claude-time-zone" aria-describedby="claude-time-zone-help claude-time-zone-scope"
      value={draft} disabled={!stored || saving} onChange={event => { setDraft(event.target.value); setSaved(false); setError('') }}>
      <option value="system">{t('跟随系统', 'Follow system')}{stored ? ` (${stored.systemTimeZone})` : ''}</option>
      {zones.map(zone => <option key={zone} value={zone}>{zone}</option>)}
    </select>
    {preview && <p className="claude-time-zone-settings__preview">{t('当前时间', 'Current time')}：{preview} · {effective}</p>}
    <p id="claude-time-zone-scope">{t(
      '仅影响码头启动的 Claude Code 及其子进程。保存后，新建、恢复和重启的会话自动使用该时区。正在运行的会话需重启后生效；保存不会中断任务。',
      'Applies only to Claude Code and its child processes launched by Matou. New, resumed and restarted sessions use the saved zone automatically. Running sessions need a restart; saving does not interrupt tasks.'
    )}</p>
    <div className="claude-time-zone-settings__actions">
      <button type="button" disabled={!stored || saving || draft === stored.timeZone && !error} onClick={() => void save()}>
        {saving ? t('保存中…', 'Saving…') : t('保存', 'Save')}
      </button>
      {saved && <span role="status">{t('已保存，下次启动 Claude Code 时自动生效。', 'Saved. Applies automatically on the next Claude Code launch.')}</span>}
    </div>
    {error && <div role="alert">{error}{!stored && <button type="button" onClick={() => setRetry(value => value + 1)}>{t('重试', 'Retry')}</button>}</div>}
  </div>
}
