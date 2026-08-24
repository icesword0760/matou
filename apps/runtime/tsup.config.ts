import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['src/index.ts'],
  format: ['cjs'],
  outExtension: () => ({ js: '.cjs' }),
  platform: 'node',
  target: 'node22',
  sourcemap: true,
  clean: true,
  external: ['node-pty'],
  noExternal: ['@matou/contracts', '@matou/domain', 'zod']
})
