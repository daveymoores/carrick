// Ambient stubs — keep types resolvable without npm install

declare module "express" {
  export interface Request {
    params: Record<string, string>;
    body: unknown;
    query: Record<string, string>;
  }
  export interface Response {
    status(code: number): Response;
    json(body: unknown): Response;
    send(body: unknown): Response;
  }
  export interface NextFunction {
    (err?: unknown): void;
  }
  export interface Application {
    get(path: string, handler: (req: Request, res: Response) => void): Application;
    post(path: string, handler: (req: Request, res: Response) => void): Application;
    listen(port: number, callback?: () => void): void;
  }
  function express(): Application;
  export default express;
}

declare module "axios" {
  export interface AxiosResponse<T = unknown> {
    data: T;
    status: number;
    statusText: string;
  }
  export interface AxiosInstance {
    get<T = unknown>(url: string, config?: unknown): Promise<AxiosResponse<T>>;
    post<T = unknown>(url: string, data?: unknown, config?: unknown): Promise<AxiosResponse<T>>;
  }
  const axios: AxiosInstance;
  export default axios;
}

// kafkajs stub — producer.send (publish, edge 2) + admin().createTopics (DECOY).
declare module "kafkajs" {
  export interface Message {
    key?: string;
    value: string | Buffer;
  }
  export interface ProducerRecord {
    topic: string;
    messages: Message[];
  }
  export interface Producer {
    connect(): Promise<void>;
    send(record: ProducerRecord): Promise<void>;
  }
  export interface Admin {
    connect(): Promise<void>;
    createTopics(config: { topics: { topic: string }[] }): Promise<boolean>;
  }
  export class Kafka {
    constructor(config: { clientId: string; brokers: string[] });
    producer(): Producer;
    admin(): Admin;
  }
}

// nats.js stub — nc.publish (publish, orphan) + JSONCodec encode.
declare module "nats" {
  export interface Codec<T> {
    encode(value: T): Uint8Array;
    decode(data: Uint8Array): T;
  }
  export function JSONCodec<T = unknown>(): Codec<T>;
  export interface NatsConnection {
    publish(subject: string, payload: Uint8Array): void;
    drain(): Promise<void>;
  }
  export function connect(opts: { servers: string }): Promise<NatsConnection>;
}
