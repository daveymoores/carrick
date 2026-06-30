// GraphQL consumer over graphql-ws using TypedDocumentNode codegen documents.
//
// Both edges are CONSUMER side (web-dashboard consumes the orders-engine Pothos
// schema). They are tagged `roadmap`: the scanner detects gql-tagged templates,
// not TypedDocumentNode exports + wsClient.subscribe(payload, sink), so neither
// the document detection nor the result-type anchor join fires today (#268-class
// gap generalized to graphql-ws).
//
//   client.query({ query: OrderDocument })            -> graphql|query|order        [edge 5]
//   wsClient.subscribe({ query: OrderEventsDocument }) -> graphql|subscription|orderEvents [edge 6]

import { createClient } from "graphql-ws";
import { OrderDocument, OrderEventsDocument } from "../graphql/generated";
import type { OrderView, OrderEvent } from "../graphql/generated";

const GQL_WS_URL = process.env.GATEWAY_GQL_WS_URL ?? "";
const wsClient = createClient({ url: GQL_WS_URL });

// query order — one-shot over the ws client; result type rides on OrderDocument.
export async function fetchOrder(): Promise<OrderView> {
  const res = await wsClient.query<{ order: OrderView }>({
    query: OrderDocument,
  });
  return res.data.order;
}

// subscription orderEvents — streaming consumer. The sink receives OrderEvent,
// whose `kind` union includes "cancelled" (the incompatible extra member).
export function onOrderEvents(handler: (e: OrderEvent) => void): () => void {
  return wsClient.subscribe<{ orderEvents: OrderEvent }>(
    { query: OrderEventsDocument },
    {
      next: (value) => handler(value.orderEvents),
      error: () => {},
      complete: () => {},
    }
  );
}
