import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['src/**/*.test.{ts,tsx}'],
    exclude: ['src/**/*.performance.test.{ts,tsx}'],
    fileParallelism: false,
    maxWorkers: 1
  }
})
