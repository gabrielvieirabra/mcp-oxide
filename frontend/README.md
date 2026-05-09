# mcp-oxide Console

React + TypeScript frontend for the mcp-oxide gateway.

## Commands

```bash
npm install
npm run dev
npm run build
npm run lint
```

The dev server proxies gateway routes to `http://localhost:8080`. Set a
different gateway URL in the console header when needed.

## Screens

- Overview: health, readiness, provider summary, adapter/tool counts.
- Adapters: inventory, status, detail JSON, create/delete.
- Tools: inventory, status, detail JSON, create/delete.
- Playground: JSON-RPC requests through `/mcp` or a selected adapter.
