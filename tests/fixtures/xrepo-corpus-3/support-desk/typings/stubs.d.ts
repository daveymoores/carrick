// Ambient stubs — keep types resolvable without npm install.
// Just enough surface for the call sites; the scanner reads AST structure.

declare module "@nestjs/common" {
  export function Controller(prefix?: string): ClassDecorator;
  export function Get(path?: string): MethodDecorator;
  export function Param(name?: string): ParameterDecorator;
}

declare module "socket.io" {
  export interface ServerSocket {
    on(event: string, handler: (payload: any) => void): void;
    emit(event: string, payload: unknown): void;
  }
  export class Server {
    constructor(port?: number);
    on(event: "connection", handler: (socket: ServerSocket) => void): void;
  }
}
