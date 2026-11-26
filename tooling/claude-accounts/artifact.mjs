import { createHash } from 'node:crypto'
import { cp, lstat, mkdir, open, readFile, readdir, readlink, rename, symlink, writeFile } from 'node:fs/promises'
import { basename, dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

import { fileDigests } from '../native-release-helpers.mjs'

const source = fileURLToPath(new URL('.', import.meta.url))
const inputs = ['bridge.py', 'isolated_browser.py', 'local_engine.py', 'pyproject.toml', 'uv.lock', 'build.mjs']
const manifestName = 'source.json'
const engineName = 'matou-claude-accounts'
const machoMagic = new Set([0xfeedface, 0xcefaedfe, 0xcffaedfe, 0xfeedfacf, 0xcafebabe, 0xcafebabf])

export async function accountEngineSourceDigest() {
  const hash = createHash('sha256')
  for (const name of inputs) {
    hash.update(JSON.stringify([name, (await fileDigests(join(source, name))).sha256]))
  }
  return hash.digest('hex')
}

async function distributionDigest(root, relocated = new Map()) {
  for (const name of [engineName, '_internal/base_library.zip']) {
    const file = await lstat(relocated.get(name) ?? join(root, name)).catch(error => {
      if (error.code === 'ENOENT' || error.code === 'ENOTDIR') return null
      throw error
    })
    if (!file?.isFile() || !file.size) throw new Error(`Missing account engine distribution file: ${join(root, name)}. Run pnpm accounts:build.`)
  }

  const hash = createHash('sha256')
  async function walk(directory, prefix = '') {
    const entries = new Map((await readdir(directory, { withFileTypes: true })).map(entry => [entry.name, entry]))
    for (const [name, file] of relocated) {
      if (dirname(join(root, name)) !== directory) continue
      const entry = basename(name)
      if (entries.has(entry)) throw new Error(`Duplicate bundled account engine file: ${join(directory, entry)}`)
      entries.set(entry, await lstat(file))
    }
    const names = [...entries.keys()].sort((left, right) => Buffer.compare(Buffer.from(left), Buffer.from(right)))
    for (const name of names) {
      if (!prefix && name === manifestName) continue
      const entry = entries.get(name)
      const key = prefix + name
      const path = relocated.get(key) ?? join(directory, name)
      if (entry.isDirectory()) {
        await walk(path, key + '/')
      } else if (entry.isSymbolicLink()) {
        hash.update(JSON.stringify([key, 'link', await readlink(path)]))
      } else if (entry.isFile()) {
        hash.update(JSON.stringify([key, 'file', (await fileDigests(path)).sha256]))
      } else {
        throw new Error(`Unsupported account engine distribution entry: ${path}`)
      }
    }
  }
  await walk(root)
  return hash.digest('hex')
}

async function readCurrentManifest(directory) {
  const manifest = join(directory, manifestName)
  const data = await readFile(manifest, 'utf8').then(JSON.parse).catch(error => {
    throw new Error(`Invalid account engine source manifest ${manifest}. Run pnpm accounts:build.`, { cause: error })
  })
  if (data?.sourceSha256 !== await accountEngineSourceDigest()) {
    throw new Error(`Stale account engine distribution: ${directory}. Run pnpm accounts:build.`)
  }
  return data
}

async function writeManifest(directory, sourceSha256, relocated) {
  const manifest = { sourceSha256, distributionSha256: await distributionDigest(directory, relocated) }
  await writeFile(join(directory, manifestName), JSON.stringify(manifest, null, 2) + '\n')
}

export async function writeAccountEngineManifest(directory, expectedSource) {
  const sourceSha256 = await accountEngineSourceDigest()
  if (expectedSource !== undefined && expectedSource !== sourceSha256) {
    throw new Error('Account engine sources changed during the build. Run pnpm accounts:build again.')
  }
  await writeManifest(directory, sourceSha256)
}

export async function verifyAccountEngineManifest(directory, relocated) {
  const manifest = await readCurrentManifest(directory)
  if (manifest.distributionSha256 !== await distributionDigest(directory, relocated)) {
    throw new Error(`Account engine distribution SHA-256 mismatch: ${directory}. Run pnpm accounts:build.`)
  }
}

export async function refreshAccountEngineManifestAfterSigning(directory, relocated) {
  const { sourceSha256 } = await readCurrentManifest(directory)
  await writeManifest(directory, sourceSha256, relocated)
}

export function macAccountEngineDirectory(contents) {
  return join(contents, 'Resources', 'claude-accounts', engineName)
}

async function isMachO(file) {
  const handle = await open(file, 'r')
  try {
    const header = Buffer.alloc(4)
    const { bytesRead } = await handle.read(header)
    return bytesRead === header.length && machoMagic.has(header.readUInt32BE())
  } finally {
    await handle.close()
  }
}

// The PyInstaller bootloader uses Contents/Frameworks as its onedir home when
// the executable lives in Contents/MacOS. Data is linked in from Resources.
export async function stageMacAccountEngine(sourceDirectory, contents) {
  const directory = macAccountEngineDirectory(contents)
  const frameworks = join(contents, 'Frameworks')
  await cp(sourceDirectory, directory, { recursive: true, verbatimSymlinks: true })
  await verifyAccountEngineManifest(directory)
  await mkdir(frameworks)
  await rename(join(directory, engineName), join(contents, 'MacOS', engineName))
  const internal = join(directory, '_internal')
  for (const entry of await readdir(internal, { withFileTypes: true })) {
    const sourcePath = join(internal, entry.name)
    const targetPath = join(frameworks, entry.name)
    if (entry.isFile() && await isMachO(sourcePath)) await rename(sourcePath, targetPath)
    else await symlink(relative(frameworks, sourcePath), targetPath)
  }
  await verifyAccountEngineManifest(directory, await macAccountEngineCode(contents))
}

export async function macAccountEngineCode(contents) {
  const executable = join(contents, 'MacOS', engineName)
  const frameworks = join(contents, 'Frameworks')
  const internal = join(macAccountEngineDirectory(contents), '_internal')
  const files = new Map([[engineName, executable]])
  // Validate the bootloader's data view in addition to the source fingerprint.
  for (const name of await readdir(internal)) {
    if (await readlink(join(frameworks, name)) !== relative(frameworks, join(internal, name))) {
      throw new Error(`Invalid bundled account engine data link: ${name}`)
    }
  }
  for (const entry of await readdir(frameworks, { withFileTypes: true })) {
    const file = join(frameworks, entry.name)
    if (entry.isFile() && await isMachO(file)) files.set(`_internal/${entry.name}`, file)
  }
  return files
}
