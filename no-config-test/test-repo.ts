import express, { Request, Response, Router } from "express";

// Types
interface User {
  id: number;
  name: string;
  role?: string; // Optional, client expects it
}

interface Comment {
  author: string;
  content: string;
}

// Handlers
const handleStats = (req: Request, res: Response) => {
  res.json({ users: 500 });
};

// Express App
const app = express();
app.use(express.json());

// Nested Routers
const apiRouter = Router();
const v1Router = Router();
const adminRouter = Router();

// Root-level endpoint
app.get("/users", (req: Request, res: Response<User[]>) => {
  res.json([{ id: 1, name: "Alice" }]); // Missing 'role'
});

// Orphaned dynamic endpoint
const dynamicRoute = "/dynamic";
app.get(dynamicRoute, (req: Request, res: Response) => {
  res.json({ dynamic: true });
});

// Nested router: /api/v1/comments
v1Router.post("/comments", (req: Request<{}, {}, Comment>, res: Response) => {
  const { author, content } = req.body;
  if (!author || !content) {
    return res.status(400).json({ error: "Missing required fields" });
  }
  res.json({ commentId: "c123", author, content });
});

// Admin router: /admin/stats
adminRouter.get("/stats", handleStats);

// Mount routers
apiRouter.use("/v1", v1Router);
app.use("/api", apiRouter);
app.use("/admin", adminRouter);

// Client Calls
const fetchUsers = async (): Promise<{ name: string; role: string }[]> => {
  const response = await fetch(`${process.env.CORE_API}/users`);
  const users: User[] = await response.json();
  return users.map((u) => ({
    name: u.name,
    role: u.role || "unknown", // Response mismatch
  }));
};

const addComment = async (comment: Comment & { extra: string }) => {
  return fetch("/api/v1/comments", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(comment), // Extra 'extra'
  });
};

const fetchNotFound = async () => {
  return fetch("/not-found"); // Non-existent
};

const ambiguousCall = async () => {
  return fetch(`${process.env.UNKNOWN_API}/data`); // Ambiguous env var
};

export default app;
