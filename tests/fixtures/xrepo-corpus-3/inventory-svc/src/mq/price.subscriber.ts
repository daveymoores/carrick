import { connect, StringCodec } from "nats";
import { PriceUpdatedEvent } from "../types/stock";

const sc = StringCodec();

// Fan-out subscriber A (ops-console is B): reprice stock valuation when the
// catalog announces a price change.
export async function watchPriceUpdates(): Promise<void> {
  const nc = await connect({ servers: process.env.NATS_URL });
  nc.subscribe("catalog.price.updated", {
    callback: (_err, msg) => {
      const evt = JSON.parse(sc.decode(msg.data)) as PriceUpdatedEvent;
      reprice(evt);
    },
  });
}

function reprice(evt: PriceUpdatedEvent): void {
  void evt;
}
