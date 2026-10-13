import { hop4 } from './chain';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      hop4(api);
    },
    { prefix: '/api/v1' },
  );
}
