import { describe, expect, it, vi } from 'vitest'

import { parseTrace, probeTrace } from './trace-probe'

describe('parseTrace', () => {
  it('extracts valid ip and country from trace body', () => {
    expect(parseTrace('fl=1\nh=api.anthropic.com\nip=169.128.1.2\nloc=US\n')).toEqual({
      ip: '169.128.1.2',
      country: 'US'
    })
  })

  it('supports valid IPv6 addresses', () => {
    expect(parseTrace('ip=2606:4700:4700::1111\nloc=JP\n')).toEqual({
      ip: '2606:4700:4700::1111',
      country: 'JP'
    })
  })

  it('treats invalid loc or invalid ip as missing fields', () => {
    expect(parseTrace('ip=not-an-ip\nloc=usa\n')).toEqual({
      ip: undefined,
      country: undefined
    })
    expect(parseTrace('ip=1.2.3.4\nloc=us\n')).toEqual({
      ip: '1.2.3.4',
      country: undefined
    })
    expect(parseTrace('ip=999.999.1.1\nloc=U1\n')).toEqual({
      ip: undefined,
      country: undefined
    })
  })
})

describe('probeTrace', () => {
  it('requests https://<host>/cdn-cgi/trace with default 5000ms timeout and returns parsed sample', async () => {
    const request = vi.fn(async () => 'fl=1\nh=api.anthropic.com\nip=169.128.1.2\nloc=US\n')

    const sample = await probeTrace('api.anthropic.com', 'http://127.0.0.1:7890', { request })

    expect(request).toHaveBeenCalledWith(
      'https://api.anthropic.com/cdn-cgi/trace',
      'http://127.0.0.1:7890',
      5000
    )
    expect(sample).toEqual({
      host: 'api.anthropic.com',
      ip: '169.128.1.2',
      country: 'US'
    })
  })

  it('maps ETIMEDOUT and timeout errors to timeout', async () => {
    const err = Object.assign(new Error('connect ETIMEDOUT'), { code: 'ETIMEDOUT' })
    const request = vi.fn(async () => { throw err })

    await expect(probeTrace('api.anthropic.com', undefined, { request })).resolves.toEqual({
      host: 'api.anthropic.com',
      error: 'timeout'
    })

    const timeoutNameErr = Object.assign(new Error('request timed out'), { name: 'TimeoutError' })
    const request2 = vi.fn(async () => { throw timeoutNameErr })

    await expect(probeTrace('api.anthropic.com', undefined, { request: request2 })).resolves.toEqual({
      host: 'api.anthropic.com',
      error: 'timeout'
    })
  })

  it('maps ECONNREFUSED to unreachable', async () => {
    const err = Object.assign(new Error('connect ECONNREFUSED 127.0.0.1:7890'), { code: 'ECONNREFUSED' })
    const request = vi.fn(async () => { throw err })

    await expect(probeTrace('api.anthropic.com', 'http://127.0.0.1:7890', { request })).resolves.toEqual({
      host: 'api.anthropic.com',
      error: 'unreachable'
    })
  })

  it('maps missing or invalid trace fields to invalid', async () => {
    const missingLoc = vi.fn(async () => 'fl=1\nip=169.128.1.2\n')
    await expect(probeTrace('api.anthropic.com', undefined, { request: missingLoc })).resolves.toEqual({
      host: 'api.anthropic.com',
      error: 'invalid'
    })

    const missingIp = vi.fn(async () => 'fl=1\nloc=US\n')
    await expect(probeTrace('api.anthropic.com', undefined, { request: missingIp })).resolves.toEqual({
      host: 'api.anthropic.com',
      error: 'invalid'
    })
  })
})
