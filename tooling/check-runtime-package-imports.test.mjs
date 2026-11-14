import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { assertRuntimePackageImports } from './check-runtime-package-imports.mjs'

async function fixture(t) {
  const parent = await mkdtemp(join(tmpdir(), 'matou-package-imports-'))
  t.after(() => rm(parent, { recursive: true, force: true }))
  const runtime = join(parent, 'runtime')
  await mkdir(runtime)
  for (const entry of ['index.cjs', 'mt-cli.cjs', 'model-router-supervisor.cjs']) {
    await writeFile(join(runtime, entry), 'require("node:fs"); require("path");\n')
  }
  return { parent, runtime }
}

test('accepts builtins and shipped node-pty without treating comments or strings as imports', async t => {
  const { runtime } = await fixture(t)
  await mkdir(join(runtime, 'node-pty/lib'), { recursive: true })
  await writeFile(join(runtime, 'node-pty/lib/index.js'), 'module.exports = {}\n')
  await writeFile(join(runtime, 'index.cjs'), `
    // require('not-an-import')
    const example = "require('also-not-an-import')";
    require('fs/promises'); require('./node-pty/lib/index.js');
  `)
  await assertRuntimePackageImports(runtime)
})

test('rejects missing modules from all entries, including dependencies available only outside the package', async t => {
  const { parent, runtime } = await fixture(t)
  await mkdir(join(parent, 'node_modules/development-only'), { recursive: true })
  await writeFile(join(parent, 'node_modules/development-only/index.js'), 'module.exports = {}\n')
  await writeFile(join(runtime, 'index.cjs'), "require('https-proxy-agent'); require('development-only');\n")
  await writeFile(join(runtime, 'mt-cli.cjs'), "require('socks-proxy-agent');\n")
  await writeFile(join(runtime, 'model-router-supervisor.cjs'), "require('missing-supervisor-module');\n")
  await assert.rejects(assertRuntimePackageImports(runtime), error => {
    for (const module of ['index.cjs: https-proxy-agent', 'index.cjs: development-only',
      'mt-cli.cjs: socks-proxy-agent', 'model-router-supervisor.cjs: missing-supervisor-module']) {
      assert.ok(error.message.includes(module), error.message)
    }
    return true
  })
})
