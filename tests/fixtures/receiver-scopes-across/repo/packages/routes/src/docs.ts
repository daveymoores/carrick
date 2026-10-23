// The package documents the routes served under its scope as data,
// including the ones a host adds through `extra`.
export const documentedRoutes = [
  {
    method: 'GET',
    path: '/api/v1/orders',
    summary: 'List orders',
    responses: { 200: { schema: { $ref: '#/components/schemas/Orders' } } },
  },
  {
    method: 'GET',
    path: '/api/v1/admin/users',
    summary: 'List users',
    responses: { 200: { schema: { $ref: '#/components/schemas/Users' } } },
  },
  {
    method: 'POST',
    path: '/api/v1/admin/users',
    summary: 'Create a user',
    responses: { 200: { schema: { $ref: '#/components/schemas/Created' } } },
  },
  {
    method: 'GET',
    path: '/api/v1/audit/events',
    summary: 'List audit events',
    responses: { 200: { schema: { $ref: '#/components/schemas/Events' } } },
  },
  {
    method: 'GET',
    path: '/api/v1/audit/events/{id}',
    summary: 'Read one audit event',
    responses: { 200: { schema: { $ref: '#/components/schemas/Event' } } },
  },
];
