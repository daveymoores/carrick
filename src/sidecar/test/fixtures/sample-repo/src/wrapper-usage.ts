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

// #336 shape: the payload type comes from an explicit generic on the CALL
// (`axios.get<Order[]>` in the live repro), so the wrapper-extracted payload
// is an ARRAY and the anchor must be its element symbol plus a depth.
declare function apiGet<T>(url: string): Promise<ApiResponse<T>>;

export async function loadWrappedUserArray() {
  const usersResponse = await apiGet<UserData[]>('/api/users');
  return usersResponse;
}

// Same array payload without a wrapper: the no-unwrap branch must anchor on
// the awaited return type's element.
declare function fetchUsersDirect(): Promise<UserData[]>;

export async function loadUserArrayDirect() {
  const users = await fetchUsersDirect();
  return users;
}
