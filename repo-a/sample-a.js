import express from "express";
import { handleProductGet } from "./sample-b.js";
import { getPost, getFriendLikes } from "./blog-service.js";

const app = express();
const router = express.Router();
const adminRouter = express.Router();

// ===== ROOT LEVEL ENDPOINTS =====

// Basic GET endpoint
app.get("/users", (req, res) => {
  res.json({ name: "John", age: 30, role: "user" });
});

// POST with different response shape
app.post("/users", (req, res) => {
  res.json({ id: 101, success: true });
});

// Parameterized route
app.get("/users/:id", (req, res) => {
  res.json({
    id: req.params.id,
    name: "User Details",
    profile: { verified: true },
  });
});

// Multiple HTTP methods on same route
app.put("/users/:id", (req, res) => {
  res.json({ updated: true });
});

// DELETE endpoint
app.delete("/users/:id", (req, res) => {
  res.status(204).send();
});

// Nested data structure in response
app.get("/dashboard", (req, res) => {
  res.json({
    stats: {
      visitors: 1000,
      conversions: 120,
    },
    topPages: [
      { url: "/home", visits: 500 },
      { url: "/about", visits: 300 },
    ],
  });
});

// From https://overreacted.io/jsx-over-the-wire/?ck_subscriber_id=2072125421
app.get("/api/likes/:postId", async (req, res) => {
  const postId = req.params.postId;
  const [post, friendLikes] = await Promise.all([
    getPost(postId),
    getFriendLikes(postId, { limit: 2 }),
  ]);
  const json = {
    totalLikeCount: post.totalLikeCount,
    isLikedByUser: post.isLikedByUser,
    friendLikes: friendLikes,
  };
  res.json(json);
});

// ===== REGULAR ROUTER =====

// Basic router endpoints
router.get("/products/:id", (req, res) => {
  res.json({
    id: parseInt(req.params.id),
    name: "Product Detail",
    inStock: true,
  });
});

// router with implicit function call
router.get("/products", handleProductGet);

// ===== ADMIN ROUTER =====

// Admin router with protected endpoints
adminRouter.get("/stats", (req, res) => {
  res.json({ users: 500, revenue: 10000 });
});

adminRouter.post("/settings", (req, res) => {
  res.json({ updated: true, timestamp: new Date().toISOString() });
});

// ===== MOUNTING ROUTERS =====

// Mount the regular router at /api
app.use("/api", router);

// Mount admin router at /admin
app.use("/admin", adminRouter);

// ===== DYNAMIC ROUTE REGISTRATION =====

// Dynamically registered route
const dynamicRoute = "/dynamic";
app.get(dynamicRoute, (req, res) => {
  res.json({ dynamic: true });
});

// Programmatically defining routes with a loop
["gold", "silver", "bronze"].forEach((tier) => {
  app.get(`/membership/${tier}`, (req, res) => {
    res.json({ tier, benefits: ["benefit1", "benefit2"] });
  });
});

// ===== CLIENT-SIDE API CALLS =====

// // Basic calls
// fetch("/users");
// fetch("/api/products");

// // Calls with method specified
// fetch("/users", { method: "POST" });
// fetch("/api/products", { method: "POST" });

// // Parameterized route calls
// fetch("/users/42");
// fetch("/api/products/512");

// // Calls with options
// fetch("/users/99", {
//   method: "PUT",
//   headers: { "Content-Type": "application/json" },
// });

// // DELETE request
// fetch("/users/123", { method: "DELETE" });

// // Call to a non-existent endpoint (should generate warning)
// fetch("/not-found");

// // Method mismatch (should generate warning)
// fetch("/dashboard", { method: "POST" });

// // Admin route calls
// fetch("/admin/stats");
// fetch("/admin/settings", { method: "POST" });

// // Dynamic route call
// fetch("/dynamic");

// // Membership tier routes
// fetch("/membership/gold");
// fetch("/membership/platinum"); // Should generate warning (platinum tier doesn't exist)

// // Nested route with method mismatch (should generate warning)
// fetch("/api/products/555", { method: "DELETE" });
