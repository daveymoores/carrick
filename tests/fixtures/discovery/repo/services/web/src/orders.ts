import { publishOrder } from "./events";

export async function placeOrder(id: string): Promise<void> {
  await publishOrder(id);
}
