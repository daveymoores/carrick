import { connect, StringCodec } from "nats";
import { PriceUpdated } from "@meridian/contracts";

const sc = StringCodec();

// Fan-out: inventory-svc and ops-console both subscribe to this subject.
export async function publishPriceUpdated(evt: PriceUpdated): Promise<void> {
  const nc = await connect({ servers: process.env.NATS_URL });
  nc.publish("catalog.price.updated", sc.encode(JSON.stringify(evt)));
  await nc.close();
}
