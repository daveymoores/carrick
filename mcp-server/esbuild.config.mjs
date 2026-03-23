import { build } from "esbuild";

await build({
  entryPoints: ["src/lambda.ts"],
  bundle: true,
  platform: "node",
  target: "node22",
  format: "cjs",
  outfile: "dist-lambda/index.js",
  minify: false,
  sourcemap: false,
  // Mark built-in Node modules as external
  external: ["node:*"],
});

console.log("Built dist-lambda/index.js");
