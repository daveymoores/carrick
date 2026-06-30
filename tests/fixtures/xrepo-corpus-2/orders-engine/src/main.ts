// NestJS host bootstrap — runtime host only, carries NO cross-repo edge.
// Exists so the repo is a believable NestJS application around the Pothos
// schema (src/schema.builder.ts) and the Kafka publisher (src/kafka/producer.ts).
// No HTTP routes are registered here, so nothing is extracted from this file.

import { Module } from "@nestjs/common";
import { NestFactory } from "@nestjs/core";
import { schema } from "./schema.builder";
import { publishOrderPlaced } from "./kafka/producer";

@Module({
  providers: [],
})
class OrdersModule {}

async function bootstrap(): Promise<void> {
  // Reference the schema + publisher so they are part of the program graph.
  void schema;
  void publishOrderPlaced;
  const app = await NestFactory.create(OrdersModule);
  await app.listen(3000);
}

void bootstrap();
