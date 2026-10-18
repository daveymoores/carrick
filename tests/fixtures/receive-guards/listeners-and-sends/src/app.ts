import { app } from "invented-app";

function onReady(): void {
  console.log("ready");
}

app.on("ready", onReady);
