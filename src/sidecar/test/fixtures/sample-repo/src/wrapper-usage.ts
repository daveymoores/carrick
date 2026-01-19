import type { ApiResponse } from 'wrapper-lib';

interface UserData {
  id: string;
  name: string;
}

interface Client {
  fetchUser(): Promise<ApiResponse<UserData>>;
}

declare const client: Client;

export async function loadWrappedUser() {
  const resp = await client.fetchUser();
  return resp.data;
}

export async function loadWrappedRaw() {
  const resp = await client.fetchUser();
  return resp;
}
