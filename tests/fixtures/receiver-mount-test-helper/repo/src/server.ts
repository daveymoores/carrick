import router from "fixture-router";
import { api } from "./api";

const app = router();
// The production mount: served under a prefix.
app.route("/v1", api);
