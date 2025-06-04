import { Router, Request, Response } from 'express';

const router = Router();

// GET /api/v1/posts
router.get('/posts', (req: Request, res: Response) => {
  res.json([
    { id: 1, title: 'First Post', content: 'Hello World' },
    { id: 2, title: 'Second Post', content: 'Another post' }
  ]);
});

// POST /api/v1/posts
router.post('/posts', (req: Request, res: Response) => {
  const { title, content } = req.body;
  res.status(201).json({ id: 3, title, content });
});

// GET /api/v1/stats
router.get('/stats', (req: Request, res: Response) => {
  res.json({ totalPosts: 2, totalUsers: 10 });
});

// DELETE /api/v1/posts/:id
router.delete('/posts/:id', (req: Request, res: Response) => {
  const { id } = req.params;
  res.json({ message: `Post ${id} deleted successfully` });
});

export default router;