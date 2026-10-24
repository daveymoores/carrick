import { Request, type RequestConfig } from "@fixture/request";
import { span } from "@fixture/trace";
import { env } from "./env";

export type RestConfig = Omit<RequestConfig, "url"> & { path: string };

// The caller's options bag, less its path, is spread into the request made
// inside the span.
export const restRequest = <T>(config: RestConfig): Promise<T> => {
  const { path, headers, ...rest } = config;
  return span(`rest ${path}`, () =>
    Request.make<T>({ ...rest, url: `${env.API_URL}${path}`, headers: { ...headers } }),
  );
};

// The caller's whole bag is the request, made inside the span.
export const sendConfig = <T>(config: RequestConfig): Promise<T> =>
  span("config request", () => Request.make<T>(config));
