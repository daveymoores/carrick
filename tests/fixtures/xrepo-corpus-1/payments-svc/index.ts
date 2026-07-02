import express from "express";
import type { Request, Response } from "express";
import type {
  Payment,
  CreatePayment,
  WidgetRequest,
  WidgetCreated,
  InvoiceRequest,
  InvoiceCreated,
} from "./src/types";

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

// POST /widgets — request body { name }; the caller may send a required superset.
async function createWidget(req: Request, res: Response): Promise<void> {
  const body = req.body as WidgetRequest;
  const widget: WidgetCreated = { id: `wid_${body.name}` };
  res.status(201).json(widget);
}

// POST /invoices — request body requires { invoiceId, amountCents }.
async function createInvoice(req: Request, res: Response): Promise<void> {
  const body = req.body as InvoiceRequest;
  const invoice: InvoiceCreated = { id: `inv_${body.invoiceId}_${body.amountCents}` };
  res.status(201).json(invoice);
}

app.post("/payments", createPayment);
app.get("/payments/:paymentId", getPayment);
app.post("/widgets", createWidget);
app.post("/invoices", createInvoice);

app.listen(3002, () => {
  console.log("payments-svc running on :3002");
});
