import { registerLeft, registerRight } from './pair';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      registerLeft(api);
      registerRight(api);
    },
    { prefix: '/api/v1' },
  );
}
