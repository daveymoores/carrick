import { Router } from "express";
import { addOrder, findOrder, listOrders } from "../store/orders-store";
import { CreateOrder, Order } from "../types";

export const ordersRouter = Router();

ordersRouter.get("/", (_req, res) => {
  const orders: Order[] = listOrders();
  res.json(orders);
});

ordersRouter.get("/:id", (req, res) => {
  const order: Order | undefined = findOrder(req.params.id);
  if (!order) {
    res.status(404).json({ error: "not found" });
    return;
  }
  res.json(order);
});

ordersRouter.post("/", (req, res) => {
  const input: CreateOrder = req.body;
  const order: Order = addOrder(input);
  res.status(201).json(order);
});
