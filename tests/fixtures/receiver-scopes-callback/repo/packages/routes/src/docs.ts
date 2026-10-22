// The package documents the routes served under its scope as data.
export const documentedRoutes = [
  {
    method: 'GET',
    path: '/api/v1/teams/{id}/notes',
    summary: 'List a team notes',
    responses: { 200: { schema: { $ref: '#/components/schemas/Notes' } } },
  },
  {
    method: 'GET',
    path: '/api/v1/teams/{id}/tags',
    summary: 'List a team tags',
    responses: { 200: { schema: { $ref: '#/components/schemas/Tags' } } },
  },
  {
    method: 'GET',
    path: '/api/v1/teams/{id}/audit',
    summary: 'Read a team audit trail',
    responses: { 200: { schema: { $ref: '#/components/schemas/Audit' } } },
  },
];
