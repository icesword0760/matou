import { mkdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou } from './matou-fixture'

// Exercise the real UI/runtime/storage while delaying or losing only the final
// activation acknowledgement. No real credentials or user sessions are used.
for (const mode of ['slow', 'lost'] as const) {
  test(`provider activation handles a ${mode} acknowledgement after persistence`, async () => {
    test.setTimeout(60_000)
    const fixture = await launchMatou({ env: { MATOU_E2E_DISPLAY: 'primary' } })
    try {
      await fixture.page.addInitScript(({ mode }) => {
        const activationIds = new Set<string>()
        const originalPost = MessagePort.prototype.postMessage
        MessagePort.prototype.postMessage = function (message: any, ...args: any[]) {
          if (message?.type === 'rpc.request' && message.method === 'provider-config.activate') activationIds.add(message.requestId)
          return originalPost.call(this, message, ...args as [any])
        }
        const descriptor = Object.getOwnPropertyDescriptor(MessagePort.prototype, 'onmessage')!
        Object.defineProperty(MessagePort.prototype, 'onmessage', {
          ...descriptor,
          set(handler) {
            descriptor.set!.call(this, handler ? function (this: MessagePort, event: MessageEvent) {
              if (event.data?.type === 'rpc.response' && activationIds.delete(event.data.requestId)) {
                if (mode === 'slow') {
                  setTimeout(() => handler.call(this, event), 11_000)
                } else {
                  handler.call(this, new MessageEvent('message', { data: {
                    type: 'rpc.error', protocolVersion: event.data.protocolVersion,
                    requestId: event.data.requestId, code: 'TIMEOUT', message: 'Activation acknowledgement lost', retryable: true
                  } }))
                }
                return
              }
              handler.call(this, event)
            } : handler)
          }
        })
      }, { mode })
      await fixture.page.reload()
      const page = fixture.page
      await page.getByRole('button', { name: '设置', exact: true }).click()
      await page.getByRole('button', { name: '新增供应商' }).click()
      await page.getByRole('button', { name: /自定义服务/ }).click()
      await page.getByRole('button', { name: '下一步' }).click()
      const dialog = page.getByRole('dialog', { name: '配置 自定义服务' })
      await dialog.getByLabel('供应商名称', { exact: true }).fill('切换验收服务')
      await dialog.getByLabel('API 地址', { exact: true }).fill('http://127.0.0.1:1/v1')
      await dialog.getByLabel('API Key', { exact: true }).fill('SYNTHETIC_LOCAL_TOKEN')
      await dialog.getByLabel('默认模型', { exact: true }).fill('fixture-model')
      await dialog.getByRole('button', { name: '仅保存', exact: true }).click()
      await page.getByRole('button', { name: '切换到 切换验收服务' }).click()
      if (mode === 'slow') {
        await expect(page.getByText('默认供应商已切换', { exact: true })).toBeVisible({ timeout: 20_000 })
      } else {
        await expect(page.getByText('切换结果待确认', { exact: true })).toBeVisible()
        await expect(page.getByText('默认供应商已切换为 切换验收服务；现有会话更新结果尚未确认，请检查会话状态。')).toBeVisible()
        await expect(page.getByRole('button', { name: '重试', exact: true })).toHaveCount(0)
      }
      await expect(page.getByRole('alert')).toHaveCount(0)
      await expect(page.getByRole('button', { name: '切换验收服务 当前配置' })).toBeDisabled()
      const evidence = resolve('docs/acceptance/evidence/provider-activation-2026-11-14')
      await mkdir(evidence, { recursive: true })
      await page.locator('.model-settings__main').screenshot({ path: `${evidence}/${mode}.png` })
    } finally { await fixture.close() }
  })
}
