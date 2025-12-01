import Koa from 'koa';
import Router from '@koa/router';

const app = new Koa();
const router = new Router();

// Endpoint 1: GET /users
router.get('/users', async (ctx) => {
  ctx.body = [{ id: 1, name: 'Alice' }];
});

// Endpoint 2: GET /users/:id
router.get('/users/:id', async (ctx) => {
  const id = ctx.params.id;
  // Simulate calling another service
  const commentsResp = await fetch(`http://comment-service/api/comments?userId=${id}`);
  const comments = await commentsResp.json();
  ctx.body = { userId: id, comments };
});

// Endpoint 3: POST /orders
router.post('/orders', async (ctx) => {
  const order = ctx.request.body;
  ctx.body = { status: 'created', order };
});

// Mount behavior in Koa (nested router)
const apiRouter = new Router({ prefix: '/api/v1' });
apiRouter.get('/status', async (ctx) => {
  ctx.body = { status: 'ok' };
});

app.use(router.routes());
app.use(apiRouter.routes());

export default app;
