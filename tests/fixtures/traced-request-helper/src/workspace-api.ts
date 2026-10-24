import { restRequest } from "./rest";

export interface Workspace {
  id: string;
  name: string;
}

export const fetchWorkspace = (id: string): Promise<Workspace> =>
  restRequest<Workspace>({ path: `/v1/workspaces/${id}`, method: "GET" });

export const renameWorkspace = (id: string, name: string): Promise<Workspace> =>
  restRequest<Workspace>({ path: `/v1/workspaces/${id}`, method: "PATCH", body: JSON.stringify({ name }) });
