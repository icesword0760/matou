import { z } from 'zod'

export const domainEventEnvelopeSchema = z.object({
  sequence: z.number().int().positive(),
  eventId: z.string().min(1),
  eventType: z.string().min(1),
  aggregateType: z.string().min(1),
  aggregateId: z.string().min(1),
  workspaceId: z.string().min(1).optional(),
  taskId: z.string().min(1).optional(),
  sessionId: z.string().min(1).optional(),
  payload: z.unknown(),
  schemaVersion: z.number().int().positive(),
  requiredTerminalSequence: z.number().int().nonnegative().optional(),
  commandId: z.string().min(1),
  causationId: z.string().min(1).optional(),
  correlationId: z.string().min(1).optional(),
  occurredAt: z.number().int().nonnegative()
})

export type DomainEventWireEnvelope = z.infer<typeof domainEventEnvelopeSchema>

export function parseDomainEventEnvelope(value: unknown): DomainEventWireEnvelope {
  return domainEventEnvelopeSchema.parse(value)
}
