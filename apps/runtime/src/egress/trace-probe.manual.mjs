import { existsSync, readFileSync } from 'node:fs'
import { registerHooks, stripTypeScriptTypes } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (
      (specifier.startsWith('./') || specifier.startsWith('../')) &&
      !/\.[a-zA-Z0-9]+$/.test(specifier) &&
      context.parentURL
    ) {
      const candidate = new URL(`${specifier}.ts`, context.parentURL)
      if (existsSync(fileURLToPath(candidate))) {
        return { url: candidate.href, shortCircuit: true }
      }
    }
    return nextResolve(specifier, context)
  },
  load(url, context, nextLoad) {
    if (url.endsWith('.ts')) {
      const source = readFileSync(fileURLToPath(url), 'utf8')
      const stripped = stripTypeScriptTypes(source, { mode: 'strip', sourceUrl: url })
      return { format: 'module', source: stripped, shortCircuit: true }
    }
    return nextLoad(url, context)
  }
})

const { hasProxyEnvironment, resolveProxyForUrl } = await import(
  pathToFileURL(fileURLToPath(new URL('./proxy-route.ts', import.meta.url))).href
)
const { probeTrace } = await import(
  pathToFileURL(fileURLToPath(new URL('./trace-probe.ts', import.meta.url))).href
)

const HOSTS = [
  { family: 'anthropic', role: 'api', host: 'api.anthropic.com' },
  { family: 'anthropic', role: 'login', host: 'claude.ai' },
  { family: 'openai', role: 'api', host: 'api.openai.com' },
  { family: 'openai', role: 'login', host: 'chatgpt.com' }
]

console.log(`hasProxyEnvironment: ${hasProxyEnvironment(process.env)}`)

const results = await Promise.all(
  HOSTS.map(async (entry) => {
    const url = `https://${entry.host}/cdn-cgi/trace`
    const proxyUrl = resolveProxyForUrl(process.env, url)
    const sample = await probeTrace(entry.host, proxyUrl)
    return { ...entry, proxyUrl: proxyUrl ?? '(direct)', sample }
  })
)

for (const item of results) {
  const { family, role, host, proxyUrl, sample } = item
  if (sample.error) {
    console.log(`[${family}:${role}] ${host} via ${proxyUrl} -> ERROR(${sample.error})`)
  } else {
    console.log(`[${family}:${role}] ${host} via ${proxyUrl} -> ip=${sample.ip} loc=${sample.country}`)
  }
}
