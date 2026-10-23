import { Hooks } from "./hooks";

// The class itself, handed to whatever reads the registry.
export const clients: Array<typeof Hooks> = [Hooks];
