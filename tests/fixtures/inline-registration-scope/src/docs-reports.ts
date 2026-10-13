// The service documents three of its report routes as data.
export const documentedRoutes = [
  {
    method: 'GET',
    path: '/api/v1/reports/{id}',
    summary: 'Read one report',
    responses: { 200: { schema: { $ref: '#/components/schemas/Report' } } },
  },
  {
    method: 'GET',
    path: '/api/v1/reports/{id}/lines',
    summary: 'List the lines of one report',
    responses: { 200: { schema: { $ref: '#/components/schemas/Lines' } } },
  },
  {
    method: 'GET',
    path: '/api/v1/reports/{id}/notes',
    summary: 'List the notes on one report',
    responses: { 200: { schema: { $ref: '#/components/schemas/Notes' } } },
  },
];
