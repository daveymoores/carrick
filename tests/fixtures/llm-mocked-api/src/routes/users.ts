import { Router } from 'express';
import { User } from '../types';

const router = Router();

router.get('/', async (_req, res) => {
  const users: User[] = await listUsers();
  res.json(users);
});

router.post('/', async (req, res) => {
  const payload = req.body as { name: string; email: string };
  const created: User = await createUser(payload);
  res.json(created);
});

async function listUsers(): Promise<User[]> {
  return [];
}

async function createUser(input: { name: string; email: string }): Promise<User> {
  return { id: 1, name: input.name, email: input.email };
}

export default router;
