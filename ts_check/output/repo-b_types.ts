import { Request, Response } from "@types/express";
interface Comment {
    author: string;
    content: string;
}
export interface User {
    id: number;
    name: string;
    role: string; // Expects role
}
export type ReqAdminStatsGet = Request;
export type ReqApiPotatoesGet = Request;
export type ReqApiProfilesGet = Request;
export type ReqApiProfilesGet = Request;
export type ReqApiV1ChatPost = Request<{}, {}, Comment>;
export type ReqApiV1ChatPost = Request<{}, {}, Comment>;
export type ReqApiV1StatsGet = Request;
export type ReqApiV1StatsGet = Request;
export type ReqUsersGet = Request;
export type ReqUsersGet = Request;
export type ResAdminStatsGet = Response;
export type ResApiPotatoesGet = Response<User[]>;
export type ResApiProfilesGet = Response;
export type ResApiProfilesGet = Response;
export type ResApiV1ChatPost = Response;
export type ResApiV1ChatPost = Response;
export type ResApiV1StatsGet = Response;
export type ResApiV1StatsGet = Response;
export type ResUsersGet = Response<User[]>;
export type ResUsersGet = Response<User[]>;




















