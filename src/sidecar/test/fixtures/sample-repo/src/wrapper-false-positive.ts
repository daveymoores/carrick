interface ApiResponse<T> {
  data: T;
}

declare const client: { fetchUser(): Promise<ApiResponse<{ id: string; name: string }>> };

export async function loadLocalWrapper() {
  const resp = await client.fetchUser();
  return resp;
}
