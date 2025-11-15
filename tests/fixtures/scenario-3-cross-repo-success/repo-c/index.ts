import axios from 'axios';

// Consumer correctly calls both other repos
async function fetchUsers() {
  const response = await axios.get('http://localhost:3000/api/users');
  return response.data;
}

async function fetchProducts() {
  const response = await axios.get('http://localhost:3001/api/products');
  return response.data;
}
