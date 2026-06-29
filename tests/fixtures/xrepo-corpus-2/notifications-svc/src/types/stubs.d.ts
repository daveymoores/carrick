// Ambient stubs — keep types resolvable without npm install.
// Just enough surface for the call sites; the scanner reads AST structure.

declare module "kafkajs" {
  export interface KafkaMessage {
    // value arrives as a Buffer; subscriber Buffer->JSON.parse unwrap.
    value: Buffer | null;
  }
  export interface EachMessagePayload {
    topic: string;
    partition: number;
    message: KafkaMessage;
  }
  export interface ConsumerSubscribeTopic {
    topic: string;
    fromBeginning?: boolean;
  }
  export interface ConsumerRunConfig {
    eachMessage: (payload: EachMessagePayload) => Promise<void>;
  }
  export interface Consumer {
    connect(): Promise<void>;
    subscribe(subscription: ConsumerSubscribeTopic): Promise<void>;
    run(config: ConsumerRunConfig): Promise<void>;
  }
  export interface ConsumerConfig {
    groupId: string;
  }
  export class Kafka {
    constructor(config: { clientId: string; brokers: string[] });
    consumer(config: ConsumerConfig): Consumer;
  }
}

declare module "nats" {
  // Subscription is an async-iterable of messages; m.data is a Uint8Array.
  export interface Msg {
    subject: string;
    data: Uint8Array;
  }
  export interface Subscription extends AsyncIterable<Msg> {}
  export interface NatsConnection {
    subscribe(subject: string): Subscription;
    close(): Promise<void>;
  }
  export function connect(opts?: { servers?: string | string[] }): Promise<NatsConnection>;
  export interface Codec<T> {
    encode(d: T): Uint8Array;
    decode(a: Uint8Array): T;
  }
  export function StringCodec(): Codec<string>;
}

declare module "fastify" {
  interface FastifyRequest {
    params: Record<string, string>;
    query: Record<string, string>;
    body: unknown;
  }
  interface FastifyReply {
    send(payload?: unknown): FastifyReply;
    status(code: number): FastifyReply;
  }
  type RouteHandler<TReply = unknown> = (
    request: FastifyRequest,
    reply: FastifyReply
  ) => Promise<TReply> | TReply;
  interface FastifyInstance {
    get<TReply = unknown>(path: string, handler: RouteHandler<TReply>): this;
    post<TReply = unknown>(path: string, handler: RouteHandler<TReply>): this;
    listen(opts: { port: number }): Promise<void>;
  }
  function fastify(): FastifyInstance;
  export = fastify;
}
