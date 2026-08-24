import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['src/index.tsx'],
  format: ['esm', 'cjs'],
  sourcemap: true,
  clean: true,
  external: ['react', 'react/jsx-runtime']
})
