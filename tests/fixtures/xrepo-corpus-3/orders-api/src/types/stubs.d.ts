// Ambient stubs — keep types resolvable without npm install.
// Just enough surface for the call sites; the scanner reads AST structure.

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

declare module "amqplib" {
  export interface ConsumeMessage {
    content: Buffer;
  }
  export interface Channel {
    assertQueue(queue: string): Promise<unknown>;
    sendToQueue(queue: string, content: Buffer): boolean;
    consume(queue: string, onMessage: (msg: ConsumeMessage | null) => void): Promise<unknown>;
    ack(msg: ConsumeMessage): void;
  }
  export interface Connection {
    createChannel(): Promise<Channel>;
  }
  export function connect(url?: string): Promise<Connection>;
}

declare module "bullmq" {
  export interface Job<T = unknown> {
    data: T;
  }
  export class Queue<T = unknown> {
    constructor(name: string, opts?: { connection?: unknown });
    add(jobName: string, data: T): Promise<unknown>;
  }
  export class Worker<T = unknown> {
    constructor(
      name: string,
      processor: (job: Job<T>) => Promise<void>,
      opts?: { connection?: unknown }
    );
  }
}

declare module "kafkajs" {
  export interface KafkaMessage {
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

declare module "@aws-sdk/client-sqs" {
  export class SQSClient {
    constructor(config: Record<string, unknown>);
    send(command: unknown): Promise<unknown>;
  }
  export class SendMessageCommand {
    constructor(input: { QueueUrl: string; MessageBody: string });
  }
}
