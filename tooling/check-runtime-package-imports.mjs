import { readFile, realpath } from 'node:fs/promises'
import { createRequire, isBuiltin } from 'node:module'
import { dirname, isAbsolute, join, relative, sep } from 'node:path'

const require = createRequire(import.meta.url)
// Reuse the parser already used by the declared tsup build dependency.
const { buildSync } = createRequire(require.resolve('tsup/package.json'))('esbuild')
const entries = ['index.cjs', 'mt-cli.cjs', 'model-router-supervisor.cjs']

export async function assertRuntimePackageImports(directory) {
  const root = await realpath(directory)
  const missing = []
  for (const entry of entries) {
    const file = join(root, entry)
    const { metafile } = buildSync({
      stdin: { contents: await readFile(file, 'utf8'), sourcefile: entry },
      bundle: true, write: false, platform: 'node', external: ['*'],
      metafile: true, logLevel: 'silent'
    })
    const modules = new Set(Object.values(metafile.inputs).flatMap(input => input.imports)
      .filter(input => input.kind === 'require-call').map(input => input.path))
    const resolveFromEntry = createRequire(file)
    for (const module of modules) {
      if (isBuiltin(module)) continue
      let shipped = false
      try {
        const resolved = await realpath(resolveFromEntry.resolve(module))
        const child = relative(root, resolved)
        shipped = child !== '..' && !child.startsWith(`..${sep}`) && !isAbsolute(child)
      } catch { /* Missing modules must fail before signing the app. */ }
      if (!shipped) missing.push(`${entry}: ${module}`)
    }
  }
  if (missing.length) throw new Error(`Runtime package has unshipped require modules:\n${missing.join('\n')}`)
}
