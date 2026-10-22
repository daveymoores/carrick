import make, { Request, Response } from "fixture-server";
import { users } from "./users";
import { withApp } from "./with-app";
import { Routes } from "./routes";
import { build } from "./build";

function ok(_request: Request, response: Response): void {
  response.send("ok");
}

// (a) the control: an instance the maker built, held in a local.
export const app = make();
app.get("/a", ok);

// (b) the app handed to a function that registers on its parameter.
users(app);

// (c) a callback's contextually typed parameter.
withApp((inner) => inner.post("/b", ok));

// (d) a field set from a constructor parameter.
new Routes(app).mount();

// (e) a factory's result, read in another module.
build().get("/d", ok);
