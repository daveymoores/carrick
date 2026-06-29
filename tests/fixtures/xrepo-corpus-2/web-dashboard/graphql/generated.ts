// Codegen artifact (graphql-codegen TypedDocumentNode preset). NO gql tag — the
// operations are pre-compiled DocumentNode values carrying their result/variable
// types in TypedDocumentNode<R, V>. The scanner's gql-tag consumer path
// (src/graphql.rs) does NOT key on these → edges 5,6 consumer-side are roadmap.
//
// The top-level field of each operation is the consumer key:
//   query GetOrder        -> order       -> graphql|query|order        [edge 5]
//   subscription OrderEvts -> orderEvents -> graphql|subscription|orderEvents [edge 6]

import type { TypedDocumentNode } from "@graphql-typed-document-node/core";

// Nested money object — compatible with the orders-engine Pothos producer.
export interface MoneyView {
  amountCents: number;
  currency: string;
}

// Consumer view returned by `query order`. Compatible with the producer
// OrderView { id; total: { amountCents; currency } }.
export interface OrderView {
  id: string;
  total: MoneyView;
}

// Consumer view delivered by `subscription orderEvents`. INCOMPATIBLE with the
// producer: the consumer union admits an EXTRA member "cancelled" that the
// producer never emits ("placed" | "shipped" only) — missing-union-member
// widening, a subtler mismatch than a string-vs-number swap.
export interface OrderEvent {
  orderId: string;
  kind: "placed" | "shipped" | "cancelled";
}

// query order — TypedDocumentNode<{ order: OrderView }, {}> (no variables shape).
export const OrderDocument: TypedDocumentNode<{ order: OrderView }, {}> = {
  __resultType: undefined,
  __variablesType: undefined,
};

// subscription orderEvents — TypedDocumentNode<{ orderEvents: OrderEvent }, {}>.
export const OrderEventsDocument: TypedDocumentNode<
  { orderEvents: OrderEvent },
  {}
> = {
  __resultType: undefined,
  __variablesType: undefined,
};
