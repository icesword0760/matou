import { z } from 'zod'

export const dispatchDefaultsSchema = z.object({
  agent: z.enum(['codex', 'agy', 'dsh']),
  model: z.string().min(1).max(64).regex(/^\S+$/u),
  effort: z.enum(['low', 'medium', 'high', 'xhigh']),
  idleMin: z.number().finite().positive().max(600),
  maxMin: z.number().finite().positive().max(600),
  askWaitMin: z.number().finite().positive().max(600),
  retries: z.number().int().min(0).max(5)
})

export type DispatchDefaults = z.infer<typeof dispatchDefaultsSchema>

export const DISPATCH_DEFAULTS: Readonly<DispatchDefaults> = {
  agent: 'codex', model: 'sol', effort: 'high',
  idleMin: 10, maxMin: 45, askWaitMin: 10, retries: 3
}

export interface DispatchConfigSnapshot {
  exists: boolean
  path: string
  defaults: DispatchDefaults
}
