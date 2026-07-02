import { gql } from "graphql-tag";
import { GraphQLClient } from "graphql-request";

// The gql URL is deliberately NOT in carrick.json internalEnvVars: GraphQL
// matches on operation identity, not the connection URL.
const client = new GraphQLClient(process.env.NEXT_PUBLIC_SUPPORT_GQL_URL ?? "http://localhost:4005/graphql");

const TICKET_QUERY = gql`
  query ticket($id: ID!) {
    ticket(id: $id) {
      id
      subject
      status
    }
  }
`;

// Result shape bound only by co-location — no call-site generic on request().
export interface TicketView {
  id: string;
  subject: string;
  status: "OPEN" | "ESCALATED" | "CLOSED";
}

export async function loadTicket(id: string): Promise<TicketView> {
  const data = await client.request(TICKET_QUERY, { id });
  return (data as { ticket: TicketView }).ticket;
}
