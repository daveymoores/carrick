import { listPosts, publishPost, createPost } from "../services/posts";
import { apiPost } from "../lib/api";

export async function loadPosts() {
  return listPosts();
}

export async function onPublish(id: string) {
  return publishPost(id);
}

export async function onCreatePost(title: string) {
  return createPost(title);
}

export async function onReport(id: string) {
  return apiPost("/reports", { postId: id });
}
