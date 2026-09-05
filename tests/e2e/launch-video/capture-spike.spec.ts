// tests/e2e/launch-video/capture-spike.spec.ts
// Run: MATOU_LAUNCH_VIDEO=1 npx playwright test tests/e2e/launch-video/capture-spike --workers=1 --reporter=line
import { execFile } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'
import { expect, test } from '@playwright/test'
import { launch, placeWindow, prepareDemo, prepareHome, prepareShopPlatform, renameTask, waitForShell, activeSurface } from '../readme-capture/demo-scene'
import { ClipRecorder } from './recorder'

const run = promisify(execFile)
const VIDEO_WINDOW = { width: 1504, height: 846 }
test.setTimeout(180_000)

test('records 10 seconds at 30fps without dropping frames', async () => {
  test.skip(process.env.MATOU_LAUNCH_VIDEO !== '1', 'set MATOU_LAUNCH_VIDEO=1')
  const root = await mkdtemp(join(tmpdir(), 'matou-spike-'))
  const home = join(root, 'home'); const workspace = join(home, 'work', 'shop-api'); const demo = join(root, 'demo')
  await prepareHome(home, demo); await prepareShopPlatform(workspace); await prepareDemo(demo)
  const app = await launch({ root, home, workspace, demo })
  try {
    const page = await app.firstWindow()
    await placeWindow(app, 1, VIDEO_WINDOW)
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    const recorder = new ClipRecorder(app, 30)
    const clip = join(root, 'spike.mp4'); const events = join(root, 'spike.events.json')
    await recorder.start(clip)
    // keep the UI busy: create tasks while recording
    for (const title of ['A 事项', 'B 事项', 'C 事项']) {
      await recorder.click(page.getByRole('button', { name: '在 shop-api 中新增事项' }), 'new-task')
      await renameTask(page, '新事项', title)
      await waitForShell(activeSurface(page))
      await recorder.hold(1500)
    }
    await recorder.waitUntil(10_000)
    const stats = await recorder.stop(events)
    console.log('stats', JSON.stringify(stats))
    expect(stats.width).toBe(VIDEO_WINDOW.width * 2)
    expect(stats.height).toBe(VIDEO_WINDOW.height * 2)
    expect(stats.dropped).toBe(0)
    const seconds = (stats.stoppedAt - stats.startedAt) / 1000
    expect(Math.abs(stats.written - Math.round(seconds * 30))).toBeLessThanOrEqual(2)
    const { stdout } = await run('ffprobe', ['-v', 'error', '-select_streams', 'v:0', '-show_entries', 'stream=width,height,pix_fmt,r_frame_rate,nb_frames', '-of', 'json', clip])
    const stream = JSON.parse(stdout).streams[0]
    expect(stream.pix_fmt).toBe('yuv420p')
    expect(stream.r_frame_rate).toBe('30/1')
    expect(Number(stream.nb_frames)).toBe(stats.written)
    const log = JSON.parse(await readFile(events, 'utf8'))
    expect(log.events.filter((e: { type: string }) => e.type === 'click')).toHaveLength(3)
    expect(log.scale).toBe(2)
  } finally {
    await app.close().catch(() => {})
    if (!process.env.MATOU_KEEP_DEMO_ROOT) await rm(root, { recursive: true, force: true })
  }
})
