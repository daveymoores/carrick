/** No method states its return type: the compiler infers each one. */
export function createClient(baseUrl: string) {
  return {
    async getSettings(project: string) {
      const response = await fetch(`${baseUrl}/projects/${project}/settings`);
      const raw = await response.json();
      return {
        installEnabled: Boolean(raw.installEnabled),
        projectEnabled: Boolean(raw.projectEnabled),
        evaluator: String(raw.evaluator),
      };
    },
  };
}

export type Client = ReturnType<typeof createClient>;
