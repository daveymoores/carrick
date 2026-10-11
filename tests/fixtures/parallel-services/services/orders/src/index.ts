import express from "express";
import { ordersRouter } from "./routes/orders";
import { customersRouter } from "./routes/customers";
import { invoicesRouter } from "./routes/invoices";

const app = express();
app.use(express.json());
app.use("/api/orders", ordersRouter);
app.use("/api/customers", customersRouter);
app.use("/api/invoices", invoicesRouter);

app.get("/health", (_req, res) => {
  res.json({ status: "ok" });
});

app.listen(4100);
