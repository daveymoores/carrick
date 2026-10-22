declare module "http-server" {
  export interface Reply {
    json(body: unknown): void;
  }
  export interface Server {
    get(path: string, handler: (request: unknown, reply: Reply) => void): void;
  }
  export function createServer(): Server;
}
