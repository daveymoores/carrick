import { createServer } from "http-server";

const app = createServer();

app.get("/job", (request, reply) => {
  reply.json({ status: "ready", attempts: 1 });
});
