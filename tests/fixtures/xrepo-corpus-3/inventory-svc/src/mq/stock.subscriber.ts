import { connect } from "amqplib";
import { StockAdjustSchema, StockAdjustCommand } from "../types/stock";

// RabbitMQ work queue: orders-api publishes stock reservations here.
export async function runStockSubscriber(): Promise<void> {
  const conn = await connect(process.env.AMQP_URL);
  const ch = await conn.createChannel();
  await ch.consume("inventory.stock.adjust", (msg) => {
    if (!msg) return;
    const cmd: StockAdjustCommand = StockAdjustSchema.parse(JSON.parse(msg.content.toString()));
    applyAdjustment(cmd);
    ch.ack(msg);
  });
}

function applyAdjustment(cmd: StockAdjustCommand): void {
  void cmd;
}
