import { http } from '@fixture/http';

export interface Session {
  token: string;
}

async function send<T>(path: string, body: unknown): Promise<T> {
  return http.post<T>(path, body);
}

export async function login(email: string, password: string): Promise<Session> {
  return send<Session>('/api/login', { email, password });
}

export async function logout(scope: string): Promise<void> {
  await send<void>('/api/logout', { scope });
  await send<void>('/api/audit', { event: 'logout', scope });
}
