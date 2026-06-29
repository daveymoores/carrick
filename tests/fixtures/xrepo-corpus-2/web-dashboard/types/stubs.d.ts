// Ambient stubs — keep the TypedDocumentNode + graphql-ws + ioredis consumer
// types resolvable without npm install (mirrors corpus-1's *.d.ts convention).

// @graphql-typed-document-node/core: a codegen artifact carries the operation's
// result (R) and variables (V) types in the document's phantom type parameters.
// No gql tag is involved — generated.ts exports plain TypedDocumentNode values.
declare module "@graphql-typed-document-node/core" {
  export interface TypedDocumentNode<Result = unknown, Variables = unknown> {
    readonly __resultType?: Result;
    readonly __variablesType?: Variables;
  }
}

// graphql-ws: the WebSocket subscription client used by lib/gqlClient.ts.
declare module "graphql-ws" {
  export interface Sink<T = unknown> {
    next(value: T): void;
    error(err: unknown): void;
    complete(): void;
  }
  export interface SubscribePayload {
    query: unknown;
    variables?: unknown;
  }
  export interface Client {
    // Non-graphql-request shape: a document object + a sink callback.
    subscribe<T = unknown>(payload: SubscribePayload, sink: Sink<T>): () => void;
    // A request-style query helper used for one-shot operations.
    query<T = unknown>(payload: SubscribePayload): Promise<{ data: T }>;
  }
  export function createClient(options: { url: string }): Client;
}

// ioredis: dual-use client — pub/sub (subscribe/on 'message') AND key-value
// (set/get). lib/realtime.ts uses the pub/sub path; only that is a contract.
declare module "ioredis" {
  export default class Redis {
    constructor(url?: string);
    subscribe(...channels: string[]): Promise<number>;
    on(event: "message", handler: (channel: string, message: string) => void): this;
    publish(channel: string, message: string): Promise<number>;
    set(key: string, value: string): Promise<"OK">;
    get(key: string): Promise<string | null>;
  }
}
