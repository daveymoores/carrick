export interface Ticket {
  id: string;
  subject: string;
  status: "OPEN" | "ESCALATED" | "CLOSED";
  priority: number;
}

// Note: no assignee — ops-console expects one; that edge is the
// missing-required-field mismatch.
export interface EscalationResult {
  ticketId: string;
  escalatedAt: string;
}

export interface SupportMessage {
  ticketId: string;
  body: string;
  sentAt: string;
}
