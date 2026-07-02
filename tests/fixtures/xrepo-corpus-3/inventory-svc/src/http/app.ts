import express from "express";
import { StockLevel } from "../types/stock";

const app = express();

app.get("/warehouses/:warehouseId/stock/:sku", (req, res) => {
  const level: StockLevel = {
    sku: req.params.sku,
    warehouseId: req.params.warehouseId,
    onHand: 120,
    reserved: 8,
  };
  res.json(level);
});

app.listen(4002);

export { app };
