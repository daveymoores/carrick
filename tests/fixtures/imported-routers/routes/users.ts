import { Router, Request, Response } from 'express';

const router = Router();

// GET /users/:id
router.get('/:id', (req: Request, res: Response) => {
  const { id } = req.params;
  res.json({ id, name: 'John Doe', email: 'john@example.com' });
});

// POST /users
router.post('/', (req: Request, res: Response) => {
  const { name, email } = req.body;
  res.status(201).json({ id: 123, name, email });
});

// GET /users
router.get('/', (req: Request, res: Response) => {
  res.json([
    { id: 1, name: 'John Doe', email: 'john@example.com' },
    { id: 2, name: 'Jane Smith', email: 'jane@example.com' }
  ]);
});

export default router;