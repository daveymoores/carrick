import { http, HttpResponse } from "msw";

// Storybook/test mocks: route *registrations* that exist only in the mock
// service worker. Not producers — must not be extracted as endpoints.
export const handlers = [
  http.get("/api/v2/promotions/:id", () =>
    HttpResponse.json({ id: "promo-1", percentOff: 10 })
  ),
];
