import Hapi from '@hapi/hapi';

const server = Hapi.server({ port: 3000, host: 'localhost' });

// Endpoint 1: GET /users — h.response(...) style
server.route({
  method: 'GET',
  path: '/users',
  handler: async (request, h) => {
    return h.response([{ id: 1, name: 'Alice' }]);
  },
});

// Endpoint 2: GET /users/{id} — bare return style, with a downstream fetch
server.route({
  method: 'GET',
  path: '/users/{id}',
  handler: async (request, h) => {
    const id = request.params.id;
    const commentsResp = await fetch(`http://comment-service/api/comments?userId=${id}`);
    const comments = await commentsResp.json();
    return { userId: id, comments };
  },
});

// Endpoint 3: POST /orders — request.payload (Hapi's name for body)
server.route({
  method: 'POST',
  path: '/orders',
  handler: async (request, h) => {
    const order = request.payload;
    return h.response({ status: 'created', order }).code(201);
  },
});

// Mount behavior in Hapi: a prefixed plugin registered on the server.
const apiV1Plugin = {
  name: 'api-v1',
  register: async (server: Hapi.Server) => {
    server.route({
      method: 'GET',
      path: '/status',
      handler: async () => ({ status: 'ok' }),
    });
  },
};

await server.register({ plugin: apiV1Plugin, routes: { prefix: '/api/v1' } });

export default server;
