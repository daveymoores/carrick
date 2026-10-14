import { registerItemRoutes } from './team-items';
import { registerNoteRoutes } from './team-notes';

// Hands the instance on twice from one line.
export function registerBothRoutes(app: any, deps: any) {
  registerItemRoutes(app, deps); registerNoteRoutes(deps, app);
}
