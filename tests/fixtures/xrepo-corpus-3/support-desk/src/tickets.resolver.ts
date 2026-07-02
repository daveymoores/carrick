import { Ticket, EscalationResult } from "./types/tickets";

// Resolvers for src/schema.graphql root fields.
export async function resolveTicket(id: string): Promise<Ticket> {
  return {
    id,
    subject: "Cracked mug on arrival",
    status: "OPEN",
    priority: 2,
  };
}

export async function resolveEscalateTicket(id: string, reason: string): Promise<EscalationResult> {
  void reason;
  return {
    ticketId: id,
    escalatedAt: "2026-07-02T12:00:00Z",
  };
}

export async function* resolveTicketUpdated(id: string): AsyncGenerator<Ticket> {
  yield await resolveTicket(id);
}
