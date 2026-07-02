import { connect } from "amqplib";
import { StockAdjustCommand } from "../types/orders";

// Reserve stock when an order is placed: RabbitMQ work queue consumed by
// inventory-svc.
export async function reserveStock(cmd: StockAdjustCommand): Promise<void> {
  const conn = await connect(process.env.AMQP_URL);
  const ch = await conn.createChannel();
  ch.sendToQueue("inventory.stock.adjust", Buffer.from(JSON.stringify(cmd)));
}
