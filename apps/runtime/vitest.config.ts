import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    // Several suites drive real shells and PTYs; shared CI runners make their
    // timing flaky. Retry there only, so a genuine regression still fails.
    retry: process.env.CI ? 2 : 0
  }
})
