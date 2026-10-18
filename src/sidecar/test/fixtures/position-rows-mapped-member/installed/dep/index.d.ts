export type Verb = "get" | "post";
export type Fetcher = (url: string) => Promise<string>;
export declare const depClient: { [K in Verb]: Fetcher };
