import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['src/**/*.test.{ts,tsx}'],
    exclude: ['src/**/*.performance.test.{ts,tsx}'],
    fileParallelism: false,
    maxWorkers: 1,
    // Shared CI runners make timing-sensitive suites flaky; retry there only.
    retry: process.env.CI ? 2 : 0
  }
})
