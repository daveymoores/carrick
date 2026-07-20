// Barrel: re-exports the payload type without declaring it. A symbol's
// declaration list can surface this ExportSpecifier — the anchor source must
// NOT point here (the bundler resolves only real declarations).
export { ReExportedPayload } from "./pubsub-barrel-types.js";
