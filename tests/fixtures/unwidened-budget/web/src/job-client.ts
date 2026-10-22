import { create } from "http-client";

const api = create({ baseURL: "http://localhost:3000" });

export interface Job {
  status: "ready";
  attempts: number;
}

export async function readJob(): Promise<Job> {
  const response = await api.get<Job>("/job");
  return response.data;
}
