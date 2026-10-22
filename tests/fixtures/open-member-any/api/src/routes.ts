import { createServer } from "http-server";

export interface Checkout {
  id: string;
  meta: any;
}

const app = createServer();

app.post("/checkout", (request, reply) => {
  const result: Checkout = { id: "a", meta: {} };
  reply.json(result);
});
