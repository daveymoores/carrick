interface UserData {
  id: string;
  name: string;
}

interface UserResponse {
  data: UserData;
}

interface Client {
  getUser(): Promise<UserResponse>;
  getClient(): { profile(): { fetch(): Promise<UserData> } };
}

declare const client: Client;

export async function loadUser() {
  const { data } = await client.getUser();
  return data;
}

export async function loadUserChain() {
  const user = await client.getClient().profile().fetch();
  return user;
}
