import type { Order } from "@acme/shapes";
import type { OrderPlaced } from "@acme/events";
import type { Ledger } from "@acme/legacy";
import type { Receipt } from "@acme/cjs";
import type { Stray } from "@acme/orphan";

export type Placed = { order: Order; placed: OrderPlaced };
export type Lost = { stray: Stray };
export type Priced = { amount: Order["total"]["amount"] };

export async function summary(order: Order, placed: OrderPlaced, ledger: Ledger, receipt: Receipt) {
  const body = {
    id: order.id,
    amount: order.total.amount,
    currency: order.total.currency,
    base: order.file.base,
    orderId: placed.orderId,
    placedAt: placed.placedAt,
    memo: ledger.entries[0].memo,
    closed: ledger.closed,
    reference: receipt.reference,
  };
  return body;
}

export function defaultsOf(ledger: Ledger) {
  return ledger.defaults.rounding;
}

export function stray(value: Stray) {
  const note = value.note;
  return value;
}
