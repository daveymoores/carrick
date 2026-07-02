import { connect } from "amqplib";

// Queue topology setup — administration, not consumption. Must not be
// extracted as a pub/sub operation.
export async function ensureQueues(): Promise<void> {
  const conn = await connect(process.env.AMQP_URL);
  const ch = await conn.createChannel();
  await ch.assertQueue("inventory.stock.adjust");
  await ch.assertQueue("inventory.dead-letter");
}
