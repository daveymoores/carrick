import express, { Request, Response } from "express";

// Types
interface Comment {
  id: string;
  authorId: number;
  content: string;
}

interface User {
  id: number;
  name: string;
}

interface UserProfile {
  userId: number;
  user: User;
  orders: any[];
  comments: Comment[];
}

// Express App
const app = express();
app.use(express.json());

// API endpoint that returns comments
app.get(
  "/api/comments",
  async (req: Request<{}, {}, {}, { userId?: string }>, res: Response<Comment[]>) => {
    const userId = req.query.userId;
    const comments: Comment[] = [
      { id: "c1", authorId: parseInt(userId || "1"), content: "First comment" },
      { id: "c2", authorId: parseInt(userId || "1"), content: "Second comment" },
    ];
    res.json(comments);
  }
);

// Multiple fetch calls to the same endpoint
app.get(
  "/users/:id/profile",
  async (req: Request<{ id: string }>, res: Response<UserProfile>) => {
    const userId = Number(req.params.id);

    // First fetch call to comments endpoint
    const commentsResp1 = await fetch(
      `${process.env.COMMENT_SERVICE_URL}/api/comments?userId=${userId}`
    );
    const commentsRaw1: Comment[] = await commentsResp1.json();

    // Second fetch call to the same comments endpoint
    const commentsResp2 = await fetch(
      `${process.env.COMMENT_SERVICE_URL}/api/comments?userId=${userId}`
    );
    const commentsRaw2: Comment[] = await commentsResp2.json();

    // Third fetch call to the same comments endpoint
    const commentsResp3 = await fetch(
      `${process.env.COMMENT_SERVICE_URL}/api/comments?userId=${userId}`
    );
    const commentsRaw3: Comment[] = await commentsResp3.json();

    const userDetailsResp = await fetch(
      `${process.env.USER_SERVICE_URL}/users/${userId}`
    );
    const userDetails: User = await userDetailsResp.json();

    res.json({
      userId,
      user: userDetails,
      orders: [],
      comments: [...commentsRaw1, ...commentsRaw2, ...commentsRaw3],
    });
  }
);

export default app;