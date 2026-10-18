import worker from "./bound";

export async function smoke(): Promise<number> {
  const response = await worker.fetch(new Request("http://localhost/items"));
  return response.status;
}
