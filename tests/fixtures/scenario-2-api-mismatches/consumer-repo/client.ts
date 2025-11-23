import axios from 'axios';

const API_BASE = 'http://localhost:3000';

async function getUsers() {
  // Correct: Consumer calls GET /api/users (matches producer)
  const response = await axios.get(`${API_BASE}/api/users`);
  return response.data;
}

async function deleteUser(id: string) {
  // MISMATCH: Consumer calls DELETE but producer doesn't define this endpoint
  const response = await axios.delete(`${API_BASE}/api/users/${id}`);
  return response.data;
}

async function updateUser(id: string) {
  // MISMATCH: Consumer calls PUT but producer only defines GET for this path
  const response = await axios.put(`${API_BASE}/api/users/${id}`, { name: 'Updated' });
  return response.data;
}

async function fetchNonExistent() {
  // MISMATCH: Consumer calls endpoint that doesn't exist
  const response = await axios.get(`${API_BASE}/api/products`);
  return response.data;
}
