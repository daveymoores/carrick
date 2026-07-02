// Ambient zod stub — keeps `z.infer` computable without npm install. The
// mapped type mirrors real zod's output derivation for the subset used here
// (required object fields only; optionality lives on plain interfaces).
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
