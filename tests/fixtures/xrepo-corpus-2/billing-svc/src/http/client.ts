import axios from "axios";
import { Kafka } from "kafkajs";

export interface LedgerEntry {
  orderId: string;
  amountCents: number;
}

const LEDGER_BASE = process.env.LEDGER_URL ?? "http://localhost:4000";

// ORPHAN consumer: POST ${LEDGER_URL}/ledger/append. No producer in the corpus
// serves this route, so the call stays unmatched. LEDGER_URL is declared in
// carrick.json `internalEnvVars` so the env-var-base HTTP call is classified
// internal and can be considered for cross-repo matching at all.
export async function appendToLedger(entry: LedgerEntry): Promise<void> {
  await axios.post(`${LEDGER_BASE}/ledger/append`, entry);
}

const kafka = new Kafka({ clientId: "billing-svc", brokers: ["localhost:9092"] });

// DECOY: kafka.admin().createTopics(...) is topic MANAGEMENT, not a
// publish/subscribe data flow. It must emit NOTHING — no pub/sub op, no edge.
export async function ensureTopics(): Promise<void> {
  const admin = kafka.admin();
  await admin.connect();
  await admin.createTopics({ topics: [{ topic: "order.placed" }] });
}
