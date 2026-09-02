import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it, vi } from 'vitest'

import { installFinderQuickAction } from './finder-quick-action'

describe('installFinderQuickAction', () => {
  it('ships a Finder-only folder quick action that opens the selected folder with the App', async () => {
    const bundle = join(process.cwd(), 'build', 'finder-quick-action', '进入码头.workflow')
    expect(existsSync(join(bundle, 'Contents', 'Info.plist'))).toBe(true)
    expect(existsSync(join(bundle, 'Contents', 'Resources', 'document.wflow'))).toBe(true)
    const info = await readFile(join(bundle, 'Contents', 'Info.plist'), 'utf8')
    const workflow = await readFile(join(bundle, 'Contents', 'Resources', 'document.wflow'), 'utf8')
    expect(info).toContain('<string>public.folder</string>')
    expect(info).toContain('<string>com.apple.finder</string>')
    expect(workflow).toContain('/usr/bin/open -b com.matou.desktop')
    expect(workflow).toContain('inputMethod</key>')
    expect(workflow).toContain('<integer>1</integer>')
  })

  it('includes the Finder quick action in packaged applications', async () => {
    const packageJson = JSON.parse(await readFile(join(process.cwd(), 'package.json'), 'utf8')) as {
      build?: {
        extraResources?: Array<{ from?: string; to?: string }>
        mac?: { extendInfo?: { CFBundleDocumentTypes?: Array<{ LSItemContentTypes?: string[] }> } }
      }
    }
    expect(packageJson.build?.extraResources).toContainEqual({
      from: 'build/finder-quick-action/进入码头.workflow',
      to: 'finder-quick-action/进入码头.workflow'
    })
    expect(packageJson.build?.mac?.extendInfo?.CFBundleDocumentTypes).toContainEqual(
      expect.objectContaining({ LSItemContentTypes: ['public.folder'] })
    )
  })

  it('installs the bundled Finder quick action and refreshes macOS Services', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-finder-action-'))
    const source = join(root, 'source.workflow')
    const home = join(root, 'home')
    await mkdir(join(source, 'Contents', 'Resources'), { recursive: true })
    await writeFile(join(source, 'Contents', 'Info.plist'), 'finder action')
    const refreshServices = vi.fn().mockResolvedValue(undefined)

    const installed = await installFinderQuickAction({
      platform: 'darwin', sourcePath: source, homeDirectory: home, refreshServices
    })

    expect(installed).toBe(join(home, 'Library', 'Services', '进入码头.workflow'))
    expect(await readFile(join(installed!, 'Contents', 'Info.plist'), 'utf8')).toBe('finder action')
    expect(refreshServices).toHaveBeenCalledOnce()
  })

  it('replaces an older quick action during an App update', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-finder-action-update-'))
    const source = join(root, 'source.workflow')
    const home = join(root, 'home')
    const destination = join(home, 'Library', 'Services', '进入码头.workflow')
    await mkdir(join(source, 'Contents'), { recursive: true })
    await writeFile(join(source, 'Contents', 'Info.plist'), 'new')
    await mkdir(join(destination, 'Contents'), { recursive: true })
    await writeFile(join(destination, 'Contents', 'Info.plist'), 'old')

    await installFinderQuickAction({
      platform: 'darwin', sourcePath: source, homeDirectory: home,
      refreshServices: async () => {}
    })

    expect(await readFile(join(destination, 'Contents', 'Info.plist'), 'utf8')).toBe('new')
  })

  it('leaves other operating systems unchanged', async () => {
    const refreshServices = vi.fn()
    const installed = await installFinderQuickAction({
      platform: 'linux', sourcePath: '/missing', homeDirectory: '/tmp/home', refreshServices
    })
    expect(installed).toBeUndefined()
    expect(refreshServices).not.toHaveBeenCalled()
  })
})
