import express from 'express';

const app = express();

app.get('/api/products', (req, res) => {
  res.json({ products: [] });
});

app.listen(3001);
