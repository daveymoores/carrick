import { Router } from "express";
import { Customer } from "../types";

export const customersRouter = Router();

const customers: Customer[] = [];

customersRouter.get("/", (_req, res) => {
  res.json(customers);
});

customersRouter.get("/:id", (req, res) => {
  const customer: Customer | undefined = customers.find((c) => c.id === req.params.id);
  res.json(customer ?? null);
});
