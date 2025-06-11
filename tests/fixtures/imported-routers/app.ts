import express from 'express';
import userRouter from './routes/users';
import apiRouter from './routes/api';
import healthRouter from './routes/health';

const app = express();
app.use(express.json());

// Mount imported routers
app.use('/users', userRouter);
app.use('/api/v1', apiRouter);
app.use('/health', healthRouter);

export default app;