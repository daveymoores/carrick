import Router from "@koa/router";
import { Product, VariantDetail } from "@meridian/contracts";
import { loadProduct, loadVariant, applyProductPatch, removeProduct } from "./store";

export const productsRouter = new Router();

// v2 only — the storefront's legacy v1 client was never migrated and must not
// resolve against these routes.
productsRouter.get("/api/v2/products/:id", async (ctx) => {
  const product: Product = await loadProduct(ctx.params.id);
  ctx.body = product;
});

productsRouter.get("/api/v2/products/:id/variants/:variantId", async (ctx) => {
  const variant: VariantDetail = await loadVariant(ctx.params.id, ctx.params.variantId);
  ctx.body = variant;
});

productsRouter.patch("/api/v2/products/:id", async (ctx) => {
  const updated: Product = await applyProductPatch(ctx.params.id, ctx.request.body);
  ctx.body = updated;
});

productsRouter.del("/api/v2/products/:id", async (ctx) => {
  await removeProduct(ctx.params.id);
  ctx.status = 204;
});
