import Koa from "koa";
import { productsRouter } from "./products.routes";

const app = new Koa();
app.use(productsRouter.routes());
app.use(productsRouter.allowedMethods());

app.listen(4001);

export { app };
