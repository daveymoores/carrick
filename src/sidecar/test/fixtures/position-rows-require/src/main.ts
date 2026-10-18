const { fetchIt } = require("dep");
const { other: renamed } = require("dep");
const dep = require("dep");
const helpers = require("./helpers");
const picked = require("dep").limit;
const { nested: { deep } } = require("dep");
const { withDefault = 1 } = require("dep");

export const a = fetchIt("x");
export const b = renamed(1);
export const c = dep.fetchIt("y");
export const d = helpers.help(2);
export const e = picked;
export const f = [deep, withDefault, dep, helpers];
