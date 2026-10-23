import type { ApiClient } from "./api-client.js";

export type MakeClient = (projectId: string) => ApiClient;

export type OpenClient = (projectId: string) => Promise<ApiClient>;

export interface Deps {
  make: (projectId: string) => ApiClient;
}
