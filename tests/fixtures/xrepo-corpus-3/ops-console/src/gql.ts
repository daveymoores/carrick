import { gql } from "graphql-tag";
import { GraphQLClient } from "graphql-request";

const client = new GraphQLClient(process.env.SUPPORT_GQL_URL ?? "http://localhost:4005/graphql");

const ESCALATE_MUTATION = gql`
  mutation escalateTicket($id: ID!, $reason: String!) {
    escalateTicket(id: $id, reason: $reason) {
      ticketId
      assignee
      escalatedAt
    }
  }
`;

// Expects an assignee the producer's EscalationResult does not carry —
// missing-required-field mismatch.
export interface EscalationReceipt {
  ticketId: string;
  assignee: string;
  escalatedAt: string;
}

export async function escalateTicket(id: string, reason: string): Promise<EscalationReceipt> {
  const data = await client.request<{ escalateTicket: EscalationReceipt }>(ESCALATE_MUTATION, {
    id,
    reason,
  });
  return data.escalateTicket;
}
