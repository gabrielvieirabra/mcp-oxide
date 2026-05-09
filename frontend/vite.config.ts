import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: false,
    proxy: {
      "/healthz": "http://localhost:8080",
      "/readyz": "http://localhost:8080",
      "/livez": "http://localhost:8080",
      "/adapters": "http://localhost:8080",
      "/tools": "http://localhost:8080",
      "/mcp": "http://localhost:8080"
    }
  },
  build: {
    target: "es2022",
    sourcemap: true,
    cssCodeSplit: true
  }
});
