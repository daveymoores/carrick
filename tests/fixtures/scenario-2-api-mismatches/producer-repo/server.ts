import express from 'express';

const app = express();

// Producer defines GET endpoint
app.get('/api/users', (req, res) => {
  res.json([
    { id: 1, name: 'Alice' },
    { id: 2, name: 'Bob' }
  ]);
});

// Producer defines POST endpoint for creating user
app.post('/api/users', (req, res) => {
  res.json({ id: 3, name: req.body.name });
});

// Producer defines GET with path parameter
app.get('/api/users/:id', (req, res) => {
  res.json({ id: req.params.id, name: 'User' });
});

app.listen(3000);
