import type { Client } from './client';

export const tools = [
  {
    name: 'settings',
    handler: (client: Client, input: { project: string }) => client.getSettings(input.project),
  },
] as const;
