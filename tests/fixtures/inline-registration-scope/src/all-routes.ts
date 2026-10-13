import { registerItemRoutes } from './team-items';

// Registers nothing itself: it hands the instance it was given on.
export function registerAllRoutes(app: any, deps: any) {
  registerItemRoutes(app, deps);
}
