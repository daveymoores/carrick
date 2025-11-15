import express from 'express';
import axios from 'axios';

const app = express();

app.get('/api/products', (req, res) => {
  res.json([{ id: 1, name: 'Product A' }]);
});

// Consumer correctly calls repo-a's endpoint
async function fetchUsers() {
  const response = await axios.get('http://localhost:3000/api/users');
  return response.data;
}

app.listen(3001);
