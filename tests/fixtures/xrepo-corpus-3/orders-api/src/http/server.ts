import fastify from "fastify";
import { CreateOrder, OrderCreated, TimelineEvent } from "../types/orders";
import { createOrder, loadTimeline } from "../store";

const app = fastify();

app.post<OrderCreated>("/orders", async (request) => {
  const order = request.body as CreateOrder;
  const created: OrderCreated = await createOrder(order);
  return created;
});

app.get<TimelineEvent[]>("/orders/:orderId/timeline", async (request) => {
  const events: TimelineEvent[] = await loadTimeline(request.params.orderId);
  return events;
});

app.listen({ port: 4003 });
