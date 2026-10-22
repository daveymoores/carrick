export const USERS_PATH = "/users";

export function listShared(): Promise<Response> {
  return fetch(USERS_PATH);
}
