import { attachHooks } from './request-hooks';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      attachHooks(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
