// Central config object: env bases are read here, not inline at call sites.
export const config = {
  ordersApiUrl: process.env.ORDERS_API_URL ?? "http://localhost:4003",
  catalogUrl: process.env.CATALOG_URL ?? "http://localhost:4001",
};
