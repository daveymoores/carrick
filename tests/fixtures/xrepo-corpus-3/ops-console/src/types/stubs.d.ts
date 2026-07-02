// Ambient stubs — keep types resolvable without npm install.
// Just enough surface for the call sites; the scanner reads AST structure.

declare module "graphql-tag" {
  export function gql(literals: TemplateStringsArray, ...args: unknown[]): unknown;
  export default gql;
}

declare module "graphql-request" {
  export class GraphQLClient {
    constructor(url: string);
    request<T = unknown>(document: unknown, variables?: Record<string, unknown>): Promise<T>;
  }
}

declare module "nats" {
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
