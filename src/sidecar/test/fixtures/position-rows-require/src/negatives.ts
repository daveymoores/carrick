function require(id: string): any {
  return { help: id };
}
const { help } = require("./helpers");
const computedName = "dep";
const { fetchIt } = require(computedName);
const gone = require("./does-not-exist");

export const g = help;
export const h = fetchIt("z");
export const i = gone.thing;
