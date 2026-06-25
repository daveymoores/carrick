// Minimal ambient shim for fastify — real types not needed; scanner reads AST structure.
declare module 'fastify' {
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

  interface FastifyPluginOptions {
    prefix?: string;
  }

  type FastifyPlugin = (
    instance: FastifyInstance,
    opts: FastifyPluginOptions,
    done: () => void
  ) => void | Promise<void>;

  interface FastifyInstance {
    get<TReply = unknown>(path: string, handler: RouteHandler<TReply>): this;
    post<TReply = unknown>(path: string, handler: RouteHandler<TReply>): this;
    put<TReply = unknown>(path: string, handler: RouteHandler<TReply>): this;
    delete<TReply = unknown>(path: string, handler: RouteHandler<TReply>): this;
    register(plugin: FastifyPlugin, opts?: FastifyPluginOptions): this;
  }

  function fastify(): FastifyInstance;
  export = fastify;
}
