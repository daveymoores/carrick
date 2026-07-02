// Ambient stubs — keep types resolvable without npm install.
// Just enough surface for the call sites; the scanner reads AST structure.

declare module "express" {
  export interface Request {
    params: Record<string, string>;
    query: Record<string, string>;
    body: unknown;
  }
  export interface Response {
    json(body: unknown): void;
    status(code: number): Response;
    send(body?: unknown): void;
  }
  export interface Application {
    get(path: string, handler: (req: Request, res: Response) => void | Promise<void>): void;
    post(path: string, handler: (req: Request, res: Response) => void | Promise<void>): void;
    listen(port: number): void;
  }
  export default function express(): Application;
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

declare module "nats" {
  export interface Msg {
    subject: string;
    data: Uint8Array;
  }
  export interface Subscription extends AsyncIterable<Msg> {}
  export interface SubscriptionOptions {
    callback: (err: unknown, msg: Msg) => void;
  }
  export interface NatsConnection {
    subscribe(subject: string, opts?: SubscriptionOptions): Subscription;
    close(): Promise<void>;
  }
  export function connect(opts?: { servers?: string | string[] }): Promise<NatsConnection>;
  export interface Codec<T> {
    encode(d: T): Uint8Array;
    decode(a: Uint8Array): T;
  }
  export function StringCodec(): Codec<string>;
}

// Ambient zod stub — keeps `z.infer` computable without npm install.
declare module "zod" {
  export interface ZodType<Output> {
    readonly _output: Output;
    parse(data: unknown): Output;
  }
  export namespace z {
    export type infer<T extends ZodType<any>> = T["_output"];
    export function object<S extends Record<string, ZodType<any>>>(
      shape: S
    ): ZodType<{ [K in keyof S]: S[K]["_output"] }>;
    export function string(): ZodType<string>;
    export function number(): ZodType<number>;
    export function boolean(): ZodType<boolean>;
  }
}
