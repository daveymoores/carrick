import { serve } from "fixture-kit";

serve((request: { url: string }) => {
  if (request.url === "/kit-health") {
    console.log("ok");
  }
});
