import { execFile } from 'node:child_process'
import { chmod, mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { expect, test, type Locator } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

const execFileAsync = promisify(execFile)

test.describe('native macOS window focus restoration', () => {
  test.skip(process.platform !== 'darwin', 'BrowserWindow focus acceptance is macOS-specific')
  test.setTimeout(60_000)

  test('restores the original terminal, Task rename, terminal search, and Fork dialog controls', async () => {
    let fixture = await launchMatou()
    await expectWindowOnColorLcd(fixture)
    const provider = join(fixture.rootDirectory, 'focus-provider.py')
    await writeFile(provider, providerScript())
    await chmod(provider, 0o755)
    await mkdir(join(fixture.rootDirectory, 'provider-inputs'))

    try {
      fixture = await restartMatou(fixture, { env: {
        MATOU_CLAUDE_COMMAND: provider,
        MATOU_FOCUS_PROVIDER_INPUT_DIR: join(fixture.rootDirectory, 'provider-inputs')
      } })
      await expectWindowOnColorLcd(fixture)
      await createNativeFocusSink(fixture)
      await expectWindowOnColorLcd(fixture)

      const terminal = fixture.page.locator(
        '.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"] .xterm-helper-textarea'
      )
      await terminal.focus()
      await cycleNativeWindowFocus(fixture, terminal)

      await fixture.page.getByRole('button', { name: '事项菜单：默认' }).click()
      await fixture.page.getByRole('menuitem', { name: '重命名' }).click()
      const renameInput = fixture.page.getByRole('textbox', { name: '事项名称' })
      await expect(renameInput).toBeFocused()
      await cycleNativeWindowFocus(fixture, renameInput)
      await fixture.page.getByRole('button', { name: '取消' }).click()

      const mod = process.platform === 'darwin' ? 'Meta' : 'Control'
      await fixture.page.keyboard.press(`${mod}+f`)
      const searchInput = fixture.page.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' })
      await expect(searchInput).toBeFocused()
      await cycleNativeWindowFocus(fixture, searchInput)
      await searchInput.press('Escape')

      await terminal.focus()
      await terminal.pressSequentially('claude')
      await terminal.press('Enter')
      const surface = fixture.page.locator(
        '.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"] .terminal-surface'
      )
      await expect(surface.locator('.xterm-rows')).toContainText('READY:focus-provider')
      await terminal.pressSequentially('ENABLE_FORK')
      await terminal.press('Enter')
      await expect(surface.locator('.xterm-rows')).toContainText('REPLY:focus-provider:ENABLE_FORK')

      const forkButton = fixture.page.getByRole('button', { name: /创建子分支/ })
      await expect(forkButton).toBeVisible()
      await forkButton.click()
      const branchInput = fixture.page.getByRole('textbox', { name: '分支名称' })
      await expect(branchInput).toBeFocused()
      await cycleNativeWindowFocus(fixture, branchInput)

      await expectWindowOnColorLcd(fixture)
    } finally {
      await fixture.close()
    }
  })
})

async function cycleNativeWindowFocus(fixture: MatouFixture, target: Locator): Promise<void> {
  const titles = await fixture.app.evaluate(({ BrowserWindow }) => {
    const windows = BrowserWindow.getAllWindows()
    const sink = windows.find((window) => window.getTitle() === 'Matou Focus Sink')
    const main = windows.find((window) => window.getTitle() !== 'Matou Focus Sink')
    if (!sink || !main) throw new Error('Expected both real BrowserWindows')
    return { sink: sink.getTitle(), main: main.getTitle() }
  })
  // The Playwright runner or provider process may temporarily become the
  // frontmost macOS application. Establish a real native main-window baseline
  // before each measured blur/focus cycle.
  await focusNativeMacWindow(fixture, titles.main)
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()
      .find((window) => window.getTitle() !== 'Matou Focus Sink')?.isFocused()
  )).toBe(true)
  await expect(target).toBeFocused()
  await focusNativeMacWindow(fixture, titles.sink)
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow }) => {
    const windows = BrowserWindow.getAllWindows()
    const main = windows.find((window) => window.getTitle() !== 'Matou Focus Sink')
    const sink = windows.find((window) => window.getTitle() === 'Matou Focus Sink')
    return main && sink ? {
      mainFocused: main.isFocused(), mainVisible: main.isVisible(),
      sinkFocused: sink.isFocused(), sinkVisible: sink.isVisible()
    } : undefined
  })).toEqual({ mainFocused: false, mainVisible: true, sinkFocused: true, sinkVisible: true })

  await focusNativeMacWindow(fixture, titles.main)
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow }) => {
    const windows = BrowserWindow.getAllWindows()
    const main = windows.find((window) => window.getTitle() !== 'Matou Focus Sink')
    const sink = windows.find((window) => window.getTitle() === 'Matou Focus Sink')
    return main && sink ? {
      mainFocused: main.isFocused(), mainVisible: main.isVisible(),
      sinkFocused: sink.isFocused(), sinkVisible: sink.isVisible()
    } : undefined
  })).toEqual({ mainFocused: true, mainVisible: true, sinkFocused: false, sinkVisible: true })
  await expect(target).toBeFocused()
}

async function focusNativeMacWindow(fixture: MatouFixture, title: string): Promise<void> {
  const pid = fixture.app.process().pid
  const appleScriptTitle = title.replaceAll('\\', '\\\\').replaceAll('"', '\\"')
  const script = `tell application "System Events"
  set targetProcess to first application process whose unix id is ${pid}
  tell targetProcess
    set frontmost to true
    tell window "${appleScriptTitle}"
      perform action "AXRaise"
      set value of attribute "AXMain" to true
      set value of attribute "AXFocused" to true
    end tell
  end tell
end tell`
  await execFileAsync('/usr/bin/osascript', ['-e', script])
}

async function createNativeFocusSink(fixture: MatouFixture): Promise<void> {
  await fixture.app.evaluate(async ({ app, BrowserWindow, screen }) => {
    const display = screen.getAllDisplays().find(({ internal }) => internal) ??
      screen.getAllDisplays().find(({ label }) => /color\s*lcd/i.test(label))
    const main = BrowserWindow.getAllWindows()[0]
    if (!display || !main) throw new Error('Expected the Color LCD and main BrowserWindow')
    const { workArea } = display
    const sink = new BrowserWindow({
      x: workArea.x + workArea.width - 340,
      y: workArea.y + workArea.height - 220,
      width: 320,
      height: 180,
      show: true,
      title: 'Matou Focus Sink'
    })
    await sink.loadURL('data:text/html,<title>Matou Focus Sink</title><main>Native focus sink</main>')
    app.focus({ steal: true })
    main.focus()
  })
}

async function expectWindowOnColorLcd(fixture: MatouFixture): Promise<void> {
  const readPlacement = () => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const displays = screen.getAllDisplays()
    // Electron may omit the human-readable display label on macOS. The
    // `internal` flag is the native identity of the built-in Color LCD and is
    // also what production E2E placement prefers over the external primary.
    const display = displays.find(({ internal }) => internal) ??
      displays.find(({ label }) => /color\s*lcd/i.test(label))
    const windows = BrowserWindow.getAllWindows()
    return display && windows.length > 0 ? {
      id: display.id,
      label: display.label,
      internal: display.internal,
      workArea: display.workArea,
      windows: windows.map((window) => ({
        bounds: window.getBounds(),
        matchedDisplayId: screen.getDisplayMatching(window.getBounds()).id,
        visible: window.isVisible()
      }))
    } : undefined
  })
  await expect.poll(async () => (await readPlacement())?.windows.every(({ visible }) => visible))
    .toBe(true)
  const placement = await readPlacement()
  expect(placement, 'Color LCD and the real Matou BrowserWindow must exist').toBeTruthy()
  expect(placement!.internal || /color\s*lcd/i.test(placement!.label)).toBe(true)
  for (const window of placement!.windows) {
    expect(window.matchedDisplayId).toBe(placement!.id)
    expect(window.bounds.x).toBeGreaterThanOrEqual(placement!.workArea.x)
    expect(window.bounds.y).toBeGreaterThanOrEqual(placement!.workArea.y)
    expect(window.bounds.x + window.bounds.width)
      .toBeLessThanOrEqual(placement!.workArea.x + placement!.workArea.width)
    expect(window.bounds.y + window.bounds.height)
      .toBeLessThanOrEqual(placement!.workArea.y + placement!.workArea.height)
  }
}

function providerScript(): string {
  return `#!/usr/bin/env python3
import json, os, pathlib, sys, urllib.request

args=sys.argv[1:]
settings=''
for index, value in enumerate(args):
    if value == '--settings' and index + 1 < len(args): settings=args[index + 1]

settings_data=json.load(open(settings))
url=settings_data['hooks']['UserPromptSubmit'][0]['hooks'][0]['url']
provider_id='focus-provider'
def hook(name):
    body=json.dumps({'hook_event_name':name,'session_id':provider_id,'cwd':os.getcwd()}).encode()
    request=urllib.request.Request(url, data=body, headers={'content-type':'application/json'}, method='POST')
    urllib.request.urlopen(request, timeout=3).read()

print('READY:' + provider_id, flush=True)
input_path=pathlib.Path(os.environ['MATOU_FOCUS_PROVIDER_INPUT_DIR']) / (provider_id + '.txt')
for line in sys.stdin:
    value=line.rstrip('\\r\\n')
    hook('UserPromptSubmit')
    with input_path.open('a') as output: output.write(value + '\\n')
    print(f'REPLY:{provider_id}:{value}', flush=True)
    hook('Stop')
`
}
