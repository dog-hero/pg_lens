import { defineConfig } from "vite";

// The bundle is served by the Rust binary (rust-embed) at the site root, so
// the default absolute `/assets/...` URLs are correct. During `vite dev`,
// API calls are proxied to a locally running `pg_lens serve`.
export default defineConfig({
  server: {
    proxy: {
      "/api": "http://127.0.0.1:8080",
    },
  },
  build: {
    // Keep the embedded payload small and auditable: no sourcemaps in the
    // binary; hashed filenames give far-future caching for free.
    sourcemap: false,
  },
});
