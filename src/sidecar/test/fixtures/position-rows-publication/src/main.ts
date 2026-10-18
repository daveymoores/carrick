import { json, renamed, type Res } from "fixture-rest";
import http from "fixture-rest";
import { makeApp, parse } from "./barrel";
import make from "fixture-make";
import start, { fork } from "fixture-ambient-module";

export function reply(res: Res): void {
  res.json({});
  json();
  renamed();
  http.get("/x");
  parse();
}

export function boot(): void {
  makeApp().post("/y", () => {});
  make();
  make.json();
  start();
  fork();
  fetch("/z");
  ambientGlobal();
  local();
}

function local(): void {}
