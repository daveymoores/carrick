import express from "express";
import type { Request, Response } from "express";
import type { Payment, CreatePayment } from "./src/types";

const app = express();

// POST /payments — create a new payment
async function createPayment(req: Request, res: Response): Promise<void> {
  const body = req.body as CreatePayment;
  const payment: Payment = {
    id: `pay_${Date.now()}`,
    orderId: body.orderId,
    amountCents: body.amountCents,
    status: "pending",
  };
  res.status(201).json(payment);
}

// GET /payments/:paymentId — retrieve a payment by ID
async function getPayment(req: Request, res: Response): Promise<void> {
  const payment: Payment = {
    id: req.params.paymentId,
    orderId: 0,
    amountCents: 0,
    status: "pending",
  };
  res.json(payment);
}

app.post("/payments", createPayment);
app.get("/payments/:paymentId", getPayment);

app.listen(3002, () => {
  console.log("payments-svc running on :3002");
});
