import axios from "axios";

export interface ChargeRequest {
  paymentId: string;
  amountCents: number;
  currency: string;
}

export interface ChargeResponse {
  chargeId: string;
  status: "accepted" | "declined";
}

const BILLING_BASE = process.env.BILLING_URL ?? "http://localhost:3003";

export async function chargePayment(
  charge: ChargeRequest
): Promise<ChargeResponse> {
  const response = await axios.post<ChargeResponse>(
    `${BILLING_BASE}/billing/charge`,
    charge
  );
  return response.data;
}
