import express from 'express';
import usersRouter from './routes/users';

const app = express();
app.use(express.json());
app.use('/api/users', usersRouter);

app.get('/health', (_req, res) => {
  res.json({ status: 'ok' });
});

app.listen(3000);
