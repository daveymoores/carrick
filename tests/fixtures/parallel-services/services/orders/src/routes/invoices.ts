import { Router } from "express";
import { findOrder } from "../store/orders-store";
import { Invoice } from "../types";
import { formatCents } from "../util/money";

export const invoicesRouter = Router();

invoicesRouter.get("/:orderId", (req, res) => {
  const order = findOrder(req.params.orderId);
  const invoice: Invoice = {
    id: `inv-${req.params.orderId}`,
    orderId: req.params.orderId,
    amount: formatCents(order?.totalCents ?? 0),
  };
  res.json(invoice);
});
