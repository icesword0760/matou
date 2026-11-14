import { defineConfig } from 'tsup'

export default defineConfig({
  entry: {
    index: 'src/index.ts',
    'mt-cli': 'src/cli/mt-cli.ts',
    'model-router-supervisor': 'src/model-router/process-supervisor.ts'
  },
  format: ['cjs'],
  outExtension: () => ({ js: '.cjs' }),
  platform: 'node',
  target: 'node22',
  sourcemap: true,
  clean: true,
  external: ['node-pty'],
  noExternal: ['@matou/contracts', '@matou/domain', '@xterm/headless', '@xterm/addon-serialize', 'zod']
})
