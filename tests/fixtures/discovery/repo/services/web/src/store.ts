import { createStore } from "fixture-store";

export const settings = createStore(() => fetch("/settings"));
