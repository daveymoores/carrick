import router from "fixture-router";
import { api } from "./api";

// An app the integration suite stands up for itself. Nothing but tests
// imports this module, though its name states no test.
export const harness = router();
harness.attach(api);
const prefixed = router();
prefixed.route("/x", api);
