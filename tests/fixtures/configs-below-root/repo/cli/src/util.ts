export async function fetchStatus(url: string): Promise<number> {
  const response = await fetch(url);
  return response.status;
}
