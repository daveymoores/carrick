// Pothos CODE-FIRST GraphQL schema (no SDL file). Root operations:
//   query order          -> OrderView      (edge 5 PRODUCER, compatible)
//   mutation cancelOrder -> CancelResult   (ORPHAN producer — no consumer doc)
//   subscription orderEvents -> OrderEvent (edge 6 PRODUCER, async-generator;
//                                           INCOMPATIBLE: producer union omits
//                                           "cancelled" that the consumer expects)
//
// Cross-repo keys: graphql|query|order, graphql|mutation|cancelOrder,
// graphql|subscription|orderEvents. Tagged `roadmap` — code-first Pothos
// detection is a scanner gap (src/graphql.rs is SDL-/gql-tag-only).

import SchemaBuilder from "@pothos/core";
import type { OrderView, OrderEvent, CancelResult } from "./types/order";

const builder = new SchemaBuilder();

const OrderViewRef = builder.objectRef<OrderView>("OrderView");
const OrderEventRef = builder.objectRef<OrderEvent>("OrderEvent");
const CancelResultRef = builder.objectRef<CancelResult>("CancelResult");

// query order -> OrderView
builder.queryType({
  fields: (t) => ({
    order: t.field({
      type: OrderViewRef,
      resolve: (): OrderView => ({
        id: "ord_1",
        total: { amountCents: 4999, currency: "EUR" },
      }),
    }),
  }),
});

// mutation cancelOrder -> CancelResult (ORPHAN producer)
builder.mutationType({
  fields: (t) => ({
    cancelOrder: t.field({
      type: CancelResultRef,
      resolve: (): CancelResult => ({ id: "ord_1", cancelled: true }),
    }),
  }),
});

// subscription orderEvents -> OrderEvent (async-generator resolver, edge 6)
builder.subscriptionType({
  fields: (t) => ({
    orderEvents: t.field({
      type: OrderEventRef,
      subscribe: async function* (): AsyncIterable<OrderEvent> {
        yield { orderId: "ord_1", kind: "placed" };
        yield { orderId: "ord_1", kind: "shipped" };
      },
      resolve: (parent: OrderEvent): OrderEvent => parent,
    }),
  }),
});

export const schema = builder.toSchema();
