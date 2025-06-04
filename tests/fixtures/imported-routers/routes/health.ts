import { Router, Request, Response } from 'express';

const router = Router();

// GET /health/status
router.get('/status', (req: Request, res: Response) => {
  res.json({ status: 'ok', timestamp: new Date().toISOString() });
});

// GET /health/ping
router.get('/ping', (req: Request, res: Response) => {
  res.json({ message: 'pong' });
});

// GET /health/ready
router.get('/ready', (req: Request, res: Response) => {
  res.json({ ready: true, uptime: process.uptime() });
});

export default router;