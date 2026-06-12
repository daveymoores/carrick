import { Router } from 'express';

const router = Router();

router.get('/summary', async () => {
  return buildSummary();
});

router.get('/export', (_req: unknown, res: { sendStatus(code: number): void }) => {
  renderReport(res);
});

function buildSummary(): { total: number } {
  return { total: 0 };
}

function renderReport(res: { sendStatus(code: number): void }): void {
  res.sendStatus(204);
}

export default router;
