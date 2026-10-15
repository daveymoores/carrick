import { defineConfig } from "@invented/devserver";

export default defineConfig({
  plugins: [
    {
      name: "mock-api",
      configureServer(server) {
        server.middlewares.use((req, res, next) => {
          if (req.url === "/api/status") {
            res.end(JSON.stringify({ ok: true }));
            return;
          }
          next();
        });
      },
    },
  ],
});
