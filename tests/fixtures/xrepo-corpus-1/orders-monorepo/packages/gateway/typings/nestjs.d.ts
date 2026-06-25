// Minimal ambient shim for @nestjs/common and @nestjs/core.
// Only the decorator factory signatures matter; the scanner reads AST structure.

declare module '@nestjs/common' {
  export function Controller(prefix?: string): ClassDecorator;
  export function Get(path?: string): MethodDecorator;
  export function Post(path?: string): MethodDecorator;
  export function Put(path?: string): MethodDecorator;
  export function Delete(path?: string): MethodDecorator;
  export function Param(param?: string): ParameterDecorator;
  export function Body(): ParameterDecorator;
  export function Query(): ParameterDecorator;
  export function Injectable(): ClassDecorator;
  export function Module(metadata: object): ClassDecorator;
}

declare module '@nestjs/core' {
  export class NestFactory {
    static create(module: any): Promise<any>;
  }
}
