import { restRequest, sendConfig } from "./rest";

export interface Workspace {
  id: string;
  name: string;
}

export const fetchWorkspace = (id: string): Promise<Workspace> =>
  restRequest<Workspace>({ path: `/v1/workspaces/${id}`, method: "GET" });

export const renameWorkspace = (id: string, name: string): Promise<Workspace> =>
  restRequest<Workspace>({ path: `/v1/workspaces/${id}`, method: "PATCH", body: JSON.stringify({ name }) });

export const removeWorkspace = (id: string): Promise<void> =>
  sendConfig<void>({ url: `/v1/workspaces/${id}`, method: "DELETE" });
