import web from "fixture-web";
import { serveRpc } from "fixture-procs";
import { rpcMiddleware } from "fixture-procs-mw";
import { appRouter } from "./router";

const app = web();

// A fetch-style adapter: served under the endpoint it is handed.
app.all("/rpc/*", (request, reply) => {
  void serveRpc({ router: appRouter, endpoint: "/rpc", request }).then((result) =>
    reply.send(result),
  );
});

// A middleware adapter mounted at the root.
app.use("/", rpcMiddleware({ router: appRouter }));

// A middleware adapter handed to a route whose path ends in a catch-all.
app.all("/api/rpc/*", rpcMiddleware({ router: appRouter }));

// A middleware adapter mounted under a prefix that ends in a catch-all.
app.use("/x/(.*)", rpcMiddleware({ router: appRouter }));
