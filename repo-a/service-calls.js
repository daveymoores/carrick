/**
 * Fetches like information for a specific post
 * @param {string} postId - The ID of the post to get likes for
 * @returns {Promise<Object>} - Like information including count and friend likes
 */
export async function fetchPostLikes(postId) {
  try {
    const response = await fetch(`/api/likes/${postId}`);

    if (!response.ok) {
      throw new Error(
        `Failed to fetch likes: ${response.status} ${response.statusText}`,
      );
    }

    const likeData = await response.json();

    // Process the data
    return {
      totalLikes: likeData.totalLikeCount,
      likedByUser: likeData.isLikedByUser,
      friendsWhoLiked: likeData.friendLikes.map((friend) => friend.name),
      friendCount: likeData.friendLikes.length,
    };
  } catch (error) {
    console.error("Error fetching post likes:", error);
    throw error;
  }
}

export async function createOrder(orderData) {
  return fetch("/orders", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      customerId: orderData.customerId,
      items: orderData.items,
      shippingAddress: orderData.shippingAddress,
    }),
  });
}

export async function addComment(postId, comment) {
  return fetch(`/blog/${postId}/comments`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      author: comment.author,
      content: comment.content,
    }),
  });
}

export async function registerForEvent(eventId, userData) {
  return fetch(`/events/${eventId}/register`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      attendeeName: userData.name,
      email: userData.email,
      // Missing ticketType field!
    }),
  });
}

export async function subscribe(userData) {
  return fetch("/newsletter/subscribe", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      emailAddress: userData.email, // Wrong field name!
      format: userData.format, // Wrong field name!
      interests: userData.topics, // Wrong field name!
    }),
  });
}

export async function processPayment(paymentData) {
  return fetch("/payments/process", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      amount: paymentData.amount,
      currency: paymentData.currency,
      // Missing nested structure for paymentMethod.type
      paymentMethod: paymentData.method,
    }),
  });
}
