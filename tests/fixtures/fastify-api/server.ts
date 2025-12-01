import fastify from 'fastify';

const app = fastify();

// Endpoint 1: GET /users
app.get('/users', async (request, reply) => {
  return [{ id: 1, name: 'Alice' }];
});

// Endpoint 2: GET /users/:id
app.get('/users/:id', async (request, reply) => {
  const { id } = request.params as { id: string };
  // Simulate calling another service
  const commentsResp = await fetch(`http://comment-service/api/comments?userId=${id}`);
  const comments = await commentsResp.json();
  return { userId: id, comments };
});

// Endpoint 3: POST /orders
app.post('/orders', async (request, reply) => {
  const order = request.body;
  return { status: 'created', order };
});

// Mount behavior in Fastify (register)
// This registers a plugin, which acts like a router mount
const apiRoutes = async (apiRoutes: any, opts: any) => {
  apiRoutes.get('/status', async () => ({ status: 'ok' }));
};

app.register(apiRoutes, { prefix: '/api/v1' });

export default app;
