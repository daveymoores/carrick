import { hop3 } from './chain';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      hop3(api);
    },
    { prefix: '/api/v1' },
  );
}
