import { t } from "./kit";

// A router held in a binding that may be set again: the walk stops there.
export let legacyRouter = t.router({
  old: t.procedure.query(() => "old"),
});

// A router bound by destructuring: the walk stops at the name.
export const { pickedRouter } = {
  pickedRouter: t.router({
    picked: t.procedure.query(() => "picked"),
  }),
};
