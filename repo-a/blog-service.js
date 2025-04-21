/**
 * Retrieves a post by its ID
 * @param {string} postId - The ID of the post to retrieve
 * @returns {Promise<Object>} The post data
 */
export async function getPost(postId) {
  // In a real app, this would fetch from a database
  // For demo purposes, we'll simulate a delay and return mock data
  await new Promise((resolve) => setTimeout(resolve, 50));

  // Mock data - in real app this would be a DB query
  return {
    id: postId,
    title: "Understanding React Hooks",
    content:
      "React Hooks provide a way to use state and lifecycle features in functional components...",
    author: "Jane Developer",
    publishDate: "2023-05-15",
    totalLikeCount: 42,
    isLikedByUser: Math.random() > 0.5, // Randomly determine if current user liked it
    tags: ["react", "javascript", "frontend"],
  };
}

/**
 * Gets likes from the user's friends for a specific post
 * @param {string} postId - The ID of the post
 * @param {Object} options - Additional options like limit
 * @returns {Promise<Array>} List of friends who liked the post
 */
export async function getFriendLikes(postId, { limit = 5 } = {}) {
  // In a real app, this would join users and likes tables
  // For demo purposes, we'll simulate a delay and return mock data
  await new Promise((resolve) => setTimeout(resolve, 75));

  // Mock data of friends who liked the post
  const allFriendLikes = [
    {
      userId: "user123",
      name: "Alex Johnson",
      profilePic: "https://example.com/profiles/alex.jpg",
      likedAt: "2023-06-01T14:32:15Z",
    },
    {
      userId: "user456",
      name: "Sam Taylor",
      profilePic: "https://example.com/profiles/sam.jpg",
      likedAt: "2023-06-01T10:15:22Z",
    },
    {
      userId: "user789",
      name: "Jordan Smith",
      profilePic: "https://example.com/profiles/jordan.jpg",
      likedAt: "2023-05-31T22:45:10Z",
    },
    {
      userId: "user101",
      name: "Casey Morgan",
      profilePic: "https://example.com/profiles/casey.jpg",
      likedAt: "2023-05-31T18:12:33Z",
    },
  ];

  // Return limited number of friend likes
  return allFriendLikes.slice(0, limit);
}
