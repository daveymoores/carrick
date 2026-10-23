import type { App } from 'fixture-plugin-server';
import { registerItemRoutes } from './team-items';

// Registers nothing itself: it hands the instance it was given on.
export function registerAllRoutes(app: App, deps: any) {
  registerItemRoutes(app, deps);
}
