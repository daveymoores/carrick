import { Request, Response } from "@types/express";
export interface User {
    id: number;
    name: string;
    role?: string; // Optional, Repo B expects it
}
export interface Order {
    customerId: string;
    items: { productId: string; quantity: number }[];
    shippingAddress: string;
}
export type ReqDynamicGet = Request;
export type ReqOrdersPost = Request<{}, {}, Order>;
export type ReqUsersGet = Request<{ id: string }>;
export type ReqUsersPut = Request<{ id: string }>;
export type ResDynamicGet = Response;
export type ResOrdersPost = Response;
export type ResUsersGet = Response<User>;
export type ResUsersPut = Response;








