// GraphQL consumer for the web frontend (consumer side of #220).
//
// `gql` tagged-template documents — scanner-detectable via
// src/graphql.rs::extract_from_ts_file (tag must be `gql` or `graphql`). The
// top-level field of each executable document is the consumer key:
//   query GetOrder   -> order        -> graphql|query|order
//   subscription ... -> orderUpdated -> graphql|subscription|orderUpdated
// These match the gateway's schema-first producers (cross-repo edge).
//
// Type-enrichment (#222): the consumer shapes deliberately diverge from the
// producer in a SUBTLE way on the subscription edge —
//   producer Order.note is OPTIONAL (note: String)
//   consumer OrderUpdate.note is REQUIRED (note: string)
// an optional-field-widening mismatch (the consumer assumes a field the
// producer may omit), distinct from the blunt `id: number` vs `string` REST
// trap already in the corpus.

import { gql } from "graphql-tag";
import { GraphQLClient } from "graphql-request";

const GATEWAY_URL = process.env.NEXT_PUBLIC_GATEWAY_GQL_URL ?? "";
const client = new GraphQLClient(GATEWAY_URL);

// Nested object on the consumer side (compatible with producer Money).
export interface MoneyView {
  amountCents: number;
  currency: string;
}

// Consumer view of an order returned by `query order`. Compatible with the
// producer Order: `note` is optional here too, so the query edge is compatible.
export interface OrderView {
  id: string;
  total: MoneyView;
  note?: string;
}

// Consumer view delivered by the `orderUpdated` subscription. INCOMPATIBLE with
// the producer: `note` is REQUIRED here but optional on the producer
// (optional-field widening). The render path below reads `update.note.length`,
// assuming the field is always present.
export interface OrderUpdate {
  id: string;
  total: MoneyView;
  note: string;
}

export const GET_ORDER = gql`
  query GetOrder($id: ID!) {
    order(id: $id) {
      id
      total {
        amountCents
        currency
      }
      note
    }
  }
`;

export const ON_ORDER_UPDATED = gql`
  subscription OnOrderUpdated {
    orderUpdated {
      id
      total {
        amountCents
        currency
      }
      note
    }
  }
`;

// query order — fetch a single order from the gateway.
export async function fetchOrderGraphql(id: string): Promise<OrderView> {
  const res = await client.request<{ order: OrderView }>(GET_ORDER, { id });
  return res.order;
}

// subscription orderUpdated — consumer assumes `note` is always present.
export function renderOrderUpdate(update: OrderUpdate): string {
  // Reads update.note directly: compiles because OrderUpdate.note is required,
  // but the producer may omit `note`, so this edge is type-INCOMPATIBLE.
  return `${update.id}: ${update.note.length} chars`;
}
