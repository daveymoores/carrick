// Ambient stubs — keep types resolvable without npm install.
// Just enough surface for the call sites in this repo; no node_modules.

// kafkajs — producer.send (publisher) + consumer.subscribe/run (DLQ decoy).
declare module "kafkajs" {
  export interface Message {
    key?: string;
    value: string | Buffer | null;
  }
  export interface ProducerRecord {
    topic: string;
    messages: Message[];
  }
  export interface Producer {
    connect(): Promise<void>;
    send(record: ProducerRecord): Promise<void>;
  }
  export interface KafkaMessage {
    value: Buffer | null;
  }
  export interface EachMessagePayload {
    topic: string;
    partition: number;
    message: KafkaMessage;
  }
  export interface Consumer {
    connect(): Promise<void>;
    subscribe(opts: { topic: string; fromBeginning?: boolean }): Promise<void>;
    run(opts: {
      eachMessage: (payload: EachMessagePayload) => Promise<void>;
    }): Promise<void>;
  }
  export class Kafka {
    constructor(config: { clientId?: string; brokers: string[] });
    producer(): Producer;
    consumer(opts: { groupId: string }): Consumer;
  }
}

// @pothos/core — code-first schema builder (objectRef + queryType/mutationType/
// subscriptionType + t.field). Generic params kept loose; resolvers carry the types.
declare module "@pothos/core" {
  export interface FieldRef<T> {
    __type: T;
  }
  export interface FieldBuilder<TParent> {
    field<T>(opts: {
      type?: unknown;
      resolve?: (parent: TParent, args: unknown) => T | Promise<T>;
      subscribe?: (parent: TParent, args: unknown) => AsyncIterable<T>;
    }): FieldRef<T>;
  }
  export interface ObjectRef<T> {
    implement(opts: {
      fields: (t: FieldBuilder<T>) => Record<string, unknown>;
    }): ObjectRef<T>;
  }
  export default class SchemaBuilder<Types = unknown> {
    objectRef<T>(name: string): ObjectRef<T>;
    queryType(opts: {
      fields: (t: FieldBuilder<unknown>) => Record<string, unknown>;
    }): void;
    mutationType(opts: {
      fields: (t: FieldBuilder<unknown>) => Record<string, unknown>;
    }): void;
    subscriptionType(opts: {
      fields: (t: FieldBuilder<unknown>) => Record<string, unknown>;
    }): void;
    toSchema(): unknown;
  }
}

// @nestjs/* — host module/injectable decorators (orders-engine runtime host).
declare module "@nestjs/common" {
  export function Module(meta: Record<string, unknown>): ClassDecorator;
  export function Injectable(): ClassDecorator;
}
declare module "@nestjs/core" {
  export class NestFactory {
    static create(module: unknown): Promise<{ listen(port: number): Promise<void> }>;
  }
}
