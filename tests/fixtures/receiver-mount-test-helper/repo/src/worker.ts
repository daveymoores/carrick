import router from "fixture-router";
import { edge } from "./edge";

const app = router();
// An entry only a test imports: the one production mount of `edge`.
app.route("/v2", edge);

export { app };
