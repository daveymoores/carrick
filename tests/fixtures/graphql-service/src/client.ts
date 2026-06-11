import { gql } from "graphql-tag";
import { request } from "graphql-request";

const ORDER_FIELDS = gql`
  fragment OrderFields on Order {
    id
    total
  }
`;

export const GET_ORDER = gql`
  query GetOrder($id: ID!) {
    order(id: $id) {
      ...OrderFields
    }
  }
  ${ORDER_FIELDS}
`;

export const GET_INVOICES = gql`
  query GetInvoices {
    invoices {
      id
    }
  }
`;

export async function fetchOrder(id: string) {
  return request("https://orders.internal/graphql", GET_ORDER, { id });
}
