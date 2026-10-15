import { createServer } from "node:http";
import { METRICS_ROUTE } from "invented-routes";
import { HEALTH_ROUTE } from "./routes";

createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  if (url.pathname === HEALTH_ROUTE) {
    res.end("ok");
    return;
  }
  if (url.pathname === METRICS_ROUTE) {
    res.end("metrics");
    return;
  }
  res.statusCode = 404;
  res.end();
}).listen(8080);
