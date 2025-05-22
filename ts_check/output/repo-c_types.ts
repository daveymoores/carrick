import { Request, Response } from "@types/express";
export interface Comment {
    author: string;
    content: string;
}
export interface Event {
    eventId: string;
    title: string;
}
export type ReqApiCommentsPost = Request<Record<string, never>, any, Comment>;
export type ReqEventsRegisterGet = Request<{ eventId: string }>;
export type ReqPostsGet = Request<{ postId: string }>;
export type ResApiCommentsPost = Response;
export type ResEventsRegisterGet = Response<Event>;
export type ResPostsGet = Response;






