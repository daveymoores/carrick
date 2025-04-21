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

// Example usage
async function displayPostLikes() {
  try {
    const postId = "post123";
    const likeInfo = await fetchPostLikes(postId);

    console.log(`This post has ${likeInfo.totalLikes} likes`);

    if (likeInfo.likedByUser) {
      console.log("You liked this post");
    }

    if (likeInfo.friendCount > 0) {
      console.log(
        `Friends who liked this: ${likeInfo.friendsWhoLiked.join(", ")}`,
      );
    }

    return likeInfo;
  } catch (error) {
    console.log("Could not display like information");
    return null;
  }
}
