import { apiGet, apiPatch, apiPost } from "../lib/api";

export interface Post { id: string; title: string }

export const listPosts = () => apiGet<Post[]>("/posts");
export const publishPost = (id: string) => apiPatch<Post>(`/posts/${id}`, { published: true });
export const createPost = (title: string) => apiPost<Post>("/posts", { title });
