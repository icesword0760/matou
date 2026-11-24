// Render individual CSS layers from the frozen bundle in a hidden window.
// Run with the matching Electron executable, outside the reference application.
const { app, BrowserWindow } = require('electron')
const { extractFile, listPackage } = require('../../node_modules/.pnpm/@electron+asar@3.4.1/node_modules/@electron/asar')
const { createHash } = require('node:crypto')
const { mkdirSync, writeFileSync } = require('node:fs')
const { resolve, join } = require('node:path')

const [archiveArg, outputArg, widthArg, heightArg, dprArg, ...flags] = process.argv.slice(2)
const archive = resolve(archiveArg)
const output = resolve(outputArg)
const width = Number(widthArg)
const height = Number(heightArg)
const dpr = Number(dprArg)
const sidebar = flags.includes('--sidebar')
const stageLeft = sidebar ? 270 : 0
const cssPath = listPackage(archive).find((path) => /^\/out\/renderer\/assets\/index-.*\.css$/.test(path))
const frozen = extractFile(archive, cssPath.slice(1))
const sheet = frozen.toString('utf8')
const selectors = [
  '.session-recovery-overlay',
  '.terminal-pane > .session-recovery-water',
  '.session-recovery-water::after',
  '.hierarchy-shell[data-theme="light"] .session-recovery-water',
]
if (sidebar) selectors.push('.hierarchy-shell[data-theme="light"] .flat-sidebar')
const rules = selectors.map((selector) => {
  const start = sheet.indexOf(`${selector} {`)
  if (start < 0) throw Error(`Frozen rule missing: ${selector}`)
  return sheet.slice(start, sheet.indexOf('}', start) + 1)
})

mkdirSync(output, { recursive: true })
for (const key of ['userData', 'sessionData', 'logs', 'crashDumps']) {
  const path = join(output, key)
  mkdirSync(path, { recursive: true })
  app.setPath(key, path)
}
app.commandLine.appendSwitch('force-device-scale-factor', String(dpr))

app.whenReady().then(async () => {
  if (process.versions.electron !== '43.4.1' || process.versions.chrome !== '150.0.7871.224') {
    throw Error('Renderer version differs from the frozen reference')
  }
  const win = new BrowserWindow({
    width: stageLeft + Math.ceil(width),
    height,
    show: false,
    frame: false,
    transparent: true,
    useContentSize: true,
    webPreferences: { backgroundThrottling: false },
  })
  const html = `<!doctype html><style>
    ${rules.join('\n')}
    html,body { margin:0 }
    .terminal-pane { position:relative; left:${stageLeft}px; width:${width}px; height:${height}px }
    .terminal-pane > .session-recovery-water { top:0 }
    .flat-sidebar { position:absolute; inset:-80px auto -80px 0; width:260px; z-index:200 }
  </style><style id="layer"></style>
  <div class="hierarchy-shell" data-theme="light"><div class="terminal-pane">
    <div class="session-recovery-overlay session-recovery-water"></div>
  </div>${sidebar ? '<aside class="flat-sidebar"></aside>' : ''}</div>`
  await win.loadURL('data:text/html;charset=utf-8,' + encodeURIComponent(html))
  const read = `(() => {
    const water = document.querySelector('.session-recovery-water')
    const {width, height} = water.getBoundingClientRect()
    return {width, height, dpr: devicePixelRatio, background: getComputedStyle(water).background,
      base: getComputedStyle(water).backgroundColor, shadow: getComputedStyle(water, '::after').boxShadow}
  })()`
  const computed = await win.webContents.executeJavaScript(read)
  if (Math.abs(computed.width - width) > 1 / 64 || computed.height !== height || computed.dpr !== dpr) {
    throw Error(`Unexpected raster scale or bounds: ${JSON.stringify(computed)}`)
  }
  const [stroke, shadow] = computed.shadow.split(/(?<=inset),\s*/)
  if (!stroke || !shadow) throw Error(`Expected two inset layers: ${computed.shadow}`)
  const noEdge = '.session-recovery-water::after { box-shadow: none }'
  const transparent = '.hierarchy-shell[data-theme="light"] .session-recovery-water { background: transparent }'
  const layers = sidebar ? {
    'sidebar-on': '',
    'sidebar-off': '.hierarchy-shell[data-theme="light"] .flat-sidebar { box-shadow: none }',
  } : {
    base: `.hierarchy-shell[data-theme="light"] .session-recovery-water { background: ${computed.base} } ${noEdge}`,
    gradient: noEdge,
    stroke: `${transparent} .session-recovery-water::after { box-shadow: ${stroke} }`,
    shadow: `${transparent} .session-recovery-water::after { box-shadow: ${shadow} }`,
    edges: transparent,
    combined: '',
  }
  for (const [name, override] of Object.entries(layers)) {
    await win.webContents.executeJavaScript(`new Promise(resolve => {
      document.querySelector('#layer').textContent = ${JSON.stringify(override)}
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    })`)
    const png = await win.webContents.capturePage(undefined, { stayHidden: true })
    writeFileSync(join(output, `${name}.png`), png.toPNG({ scaleFactor: dpr }))
  }
  writeFileSync(join(output, 'source.json'), JSON.stringify({
    electron: process.versions.electron,
    chrome: process.versions.chrome,
    bundleCssPath: cssPath,
    bundleCssSha256: createHash('sha256').update(frozen).digest('hex'),
    rules, computed, layers, stageLeft,
  }, null, 2) + '\n')
  app.exit()
}).catch((error) => {
  console.error(error)
  app.exit(1)
})
