import router from "@fixture/router";

declare const schema: object;

export function setup() {
  router.define("/reset", { method: "POST", body: schema }, async (ctx) => ctx.ok());
  router.define("/purge", { method: "DELETE", body: schema }, async (ctx) => ctx.ok());
}
