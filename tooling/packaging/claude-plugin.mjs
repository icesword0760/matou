import { lstat, readFile, readdir } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

export const claudePluginPath = 'control-assets/providers/claude-plugin'
export const claudePluginSource = fileURLToPath(new URL(`../../apps/native/runtime/${claudePluginPath}/`, import.meta.url))

const files = [
  '.claude-plugin/plugin.json',
  'skills/agent-foreman/SKILL.md',
  'skills/agent-foreman/brief-template.md',
  'skills/agent-foreman/parallel.md',
  'skills/agent-foreman/reference.md',
  'skills/mt-terminal/SKILL.md',
  'skills/mt-terminal/references/commands.md',
  'skills/mt-terminal/references/target-resolution.md'
]
const directories = new Set([...files.map(file => file.slice(0, file.lastIndexOf('/'))), 'skills'])

export async function verifyClaudePlugin(resources) {
  const plugin = join(resources, claudePluginPath)
  for (const name of ['.', ...directories]) {
    const path = join(plugin, name)
    const info = await lstat(path).catch(error => {
      if (error.code === 'ENOENT') return null
      throw error
    })
    if (!info?.isDirectory()) throw new Error(`Missing Claude plugin directory: ${path}`)
  }
  for (const file of files) {
    const path = join(plugin, file)
    const info = await lstat(path).catch(error => {
      if (error.code === 'ENOENT') return null
      throw error
    })
    if (!info?.isFile() || !info.size) throw new Error(`Missing Claude plugin asset: ${path}`)
    if (!(await readFile(path)).equals(await readFile(join(claudePluginSource, file)))) {
      throw new Error(`Claude plugin asset differs from source: ${path}`)
    }
  }
  if ((await readdir(plugin, { recursive: true })).length !== files.length + directories.size) {
    throw new Error(`Unexpected Claude plugin assets at ${plugin}`)
  }
}
