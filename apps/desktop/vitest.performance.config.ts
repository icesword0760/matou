import { defineConfig } from 'vitest/config'

// Keep wall-clock frame-budget gates out of the functional-test worker pool.
// Their measured work still uses production code and the original frame budget.
export default defineConfig({
  test: {
    include: ['src/**/*.performance.test.{ts,tsx}'],
    fileParallelism: false,
    maxWorkers: 1
  }
})
