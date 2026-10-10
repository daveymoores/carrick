import express from "express";
import { formatCents } from "./util/money";

export interface Product {
  sku: string;
  title: string;
  price: string;
}

const app = express();

app.get("/products", (_req, res) => {
  const products: Product[] = [{ sku: "a-1", title: "Kettle", price: formatCents(2599) }];
  res.json(products);
});

app.listen(4200);
