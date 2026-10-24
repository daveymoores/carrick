import router from "fixture-router";
import { jobs } from "./jobs";

// Imported by a test and by production code: the mount may serve a real app.
export const shared = router();
shared.attach(jobs);
