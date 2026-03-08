import type { User } from './types';

interface RegisterHandle {
  tag: string;
}

async function fetchUser(): Promise<User> {
  return {
    id: '1',
    name: 'Test',
    email: 'test@example.com',
    createdAt: new Date(),
  };
}

function register(handler: () => Promise<User>): RegisterHandle {
  return { tag: 'handle' };
}

register(async () => {
  const user = await fetchUser();
  return user;
});
