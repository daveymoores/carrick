// Declaring module for the barrel re-export case (carrick#413): the payload
// interface LIVES here; pubsub-barrel.ts only re-exports it.
export interface ReExportedPayload {
  shipmentId: string;
  carrier: string;
}
