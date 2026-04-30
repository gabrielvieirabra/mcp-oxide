# Smoke Testing mcp-oxide With Mock MCPs

A hand-runnable playbook that exercises **every feature from Phase 0 through
Phase 3** against the real gateway binary and three containerized
[`mock-mcp`](../../crates/testing) backends.

If you only want the in-process Rust version, run `cargo test` — the
`e2e_it.rs` integration test mirrors this file end to end.

## 1. Bring the stack up

```sh
docker compose -f deploy/smoke/docker-compose.yaml up --build -d
```

Containers:

| Service            | Role                                                | Host port |
|--------------------|-----------------------------------------------------|-----------|
| `gateway`          | mcp-oxide under test                                | `:8080`   |
| `mock-mcp-public`  | public weather tool; tagged `public`                | `:18090`  |
| `mock-mcp-admin`   | admin-only deploy tool; tagged `admin`              | `:18091`  |
| `mock-echo`        | bare MCP echo backend for adapter-proxy tests       | `:18092`  |

Config (mounted read-only into the gateway container):

- `deploy/smoke/gateway.yaml` — static-JWT identity, YAML RBAC, sqlite store,
  pre-registered `echo-static` adapter pointing at `mock-echo`.
- `deploy/smoke/policies.yaml` — full rule set for admin/viewer.
- `deploy/smoke/jwt.key` — HS256 shared secret (smoke-only).

## 2. Mint tokens

The stack uses static-JWT (HS256). Mint tokens with any JWT tool; here's
one-liners with `jq` + `openssl` that use the same secret the gateway reads
at `/etc/mcp-oxide/jwt.key`.

```sh
# Helper: mint-token <sub> <roles,comma-separated>
mint_token() {
  local sub="$1"; local roles="$2"
  local secret; secret=$(cat deploy/smoke/jwt.key | tr -d '\n')
  local now; now=$(date +%s)
  local header='{"alg":"HS256","typ":"JWT"}'
  local roles_json; roles_json=$(jq -nc --arg r "$roles" '($r | split(","))')
  local payload; payload=$(jq -nc \
    --arg sub "$sub" --arg iss "mcp-oxide-smoke" --arg aud "mcp-oxide" \
    --argjson now "$now" --argjson roles "$roles_json" \
    '{sub:$sub, iss:$iss, aud:$aud, iat:$now, exp:($now+3600), roles:$roles}')
  local h; h=$(printf %s "$header"  | openssl base64 -e -A | tr '+/' '-_' | tr -d '=')
  local p; p=$(printf %s "$payload" | openssl base64 -e -A | tr '+/' '-_' | tr -d '=')
  local sig; sig=$(printf %s "$h.$p" | openssl dgst -sha256 -hmac "$secret" -binary | openssl base64 -e -A | tr '+/' '-_' | tr -d '=')
  printf '%s.%s.%s' "$h" "$p" "$sig"
}

ADMIN=$(mint_token admin  mcp.admin)
VIEWER=$(mint_token viewer mcp.viewer)
```

## 3. Phase 0 — Health

```sh
# Provider summary
curl -s http://localhost:8080/healthz | jq

# Readiness (returns 200 if jwks+store are healthy)
curl -si http://localhost:8080/readyz
```

## 4. Phase 1 — Data plane via the static `echo-static` adapter

```sh
# No token -> 401
curl -si -X POST http://localhost:8080/adapters/echo-static/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"ping"}'

# With a valid token -> 200, proxied to mock-echo
curl -s -X POST http://localhost:8080/adapters/echo-static/mcp \
  -H "Authorization: Bearer $VIEWER" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"ping"}' | jq
```

Assert the gateway did NOT forward the caller's bearer token upstream by
inspecting the mock directly:

```sh
curl -s -X POST http://localhost:18092/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":9,"method":"ping"}' | jq
# (the mock has no way to report recorded requests over HTTP yet; use its
# container logs: `docker logs mcp-oxide-smoke-mock-echo-1`)
```

## 5. Phase 2 — Control plane CRUD + ETag

```sh
# Viewer CANNOT create -> 403
curl -si -X POST http://localhost:8080/adapters \
  -H "Authorization: Bearer $VIEWER" \
  -H 'Content-Type: application/json' \
  -d '{"name":"v-denied","image":"reg/img:1","upstream":"http://x"}'

# Admin creates an adapter pointing at mock-mcp-public -> 201 + ETag + Location
ETAG=$(curl -si -X POST http://localhost:8080/adapters \
  -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -d '{
    "name":"runtime-public",
    "image":"registry.example.com/img:1",
    "upstream":"http://mock-mcp-public:8090/mcp",
    "tags":["public"]
  }' | awk 'tolower($1)=="etag:" {print $2}' | tr -d '\r')
echo "ETag after create: $ETAG"

# Read it back
curl -s http://localhost:8080/adapters/runtime-public \
  -H "Authorization: Bearer $VIEWER" | jq

# Update with STALE If-Match -> 409
curl -si -X PUT http://localhost:8080/adapters/runtime-public \
  -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -H 'If-Match: W/"99"' \
  -d '{"description":"new"}'

# Update with FRESH If-Match -> 200, ETag changes
curl -si -X PUT http://localhost:8080/adapters/runtime-public \
  -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -H "If-Match: $ETAG" \
  -d '{"description":"rev2"}'

# Data plane to the runtime-registered adapter WITHOUT restart (Phase 2 DoD)
curl -s -X POST http://localhost:8080/adapters/runtime-public/mcp \
  -H "Authorization: Bearer $VIEWER" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | jq
```

## 6. Phase 3 — Tool Gateway Router (`POST /mcp`)

Register tools backed by each mock, then exercise list/dispatch and authz.

```sh
# Register the weather tool (public) against mock-mcp-public.
curl -s -X POST http://localhost:8080/tools \
  -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -d '{
    "name":"weather",
    "image":"registry.example.com/weather:1",
    "endpoint_port":8090,
    "endpoint_path":"/mcp",
    "tool_definition":{
      "name":"weather",
      "input_schema":{"type":"object","properties":{"location":{"type":"string"}},"required":["location"]}
    },
    "tags":["public"]
  }' | jq

# Register the deploy tool (admin-only).
curl -s -X POST http://localhost:8080/tools \
  -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -d '{
    "name":"deploy",
    "image":"registry.example.com/deploy:1",
    "endpoint_port":8090,
    "endpoint_path":"/mcp",
    "tool_definition":{"name":"deploy","input_schema":{"type":"object"}},
    "tags":["admin"]
  }' | jq

# tools/list as VIEWER → should hide `deploy`
curl -s -X POST http://localhost:8080/mcp \
  -H "Authorization: Bearer $VIEWER" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | jq '.result.tools[].name'

# tools/list as ADMIN → sees everything
curl -s -X POST http://localhost:8080/mcp \
  -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | jq '.result.tools[].name'

# tools/call for deploy as VIEWER → JSON-RPC error code -32002 (forbidden)
curl -s -X POST http://localhost:8080/mcp \
  -H "Authorization: Bearer $VIEWER" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"deploy"}}' | jq '.error.code'

# tools/call for weather as VIEWER → 200 with echoed/forwarded result
curl -s -X POST http://localhost:8080/mcp \
  -H "Authorization: Bearer $VIEWER" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"weather","arguments":{"location":"lisbon"}}}' | jq
```

## 7. Hardening regressions

```sh
# Reject path-traversal names
curl -si -X POST http://localhost:8080/adapters \
  -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"../etc/passwd","image":"reg/i:1","upstream":"http://x"}'
# -> HTTP/1.1 400 Bad Request

# Reject reserved env prefixes
curl -si -X POST http://localhost:8080/adapters \
  -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"envy","image":"reg/i:1","upstream":"http://x","env":[{"name":"AWS_ACCESS_KEY_ID","value":"x"}]}'
# -> HTTP/1.1 400 Bad Request

# Reject missing/invalid tokens consistently on the data plane
curl -si -X POST http://localhost:8080/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
# -> HTTP/1.1 401 Unauthorized
```

## 8. Tear down

```sh
docker compose -f deploy/smoke/docker-compose.yaml down -v
```

## Feature matrix covered by this playbook

| Phase | Feature                                    | Step |
|-------|--------------------------------------------|------|
| 0     | `/healthz` provider summary                | 3    |
| 0     | `/readyz`                                  | 3    |
| 1     | Data-plane auth (401 on missing token)     | 4    |
| 1     | Data-plane authz (viewer → static adapter) | 4    |
| 1     | No bearer-token leak upstream              | 4    |
| 2     | `POST /adapters` admin-only (viewer 403)   | 5    |
| 2     | Create returns `ETag` + `Location`         | 5    |
| 2     | `If-Match` stale → 409                     | 5    |
| 2     | `If-Match` fresh → 200 + new ETag          | 5    |
| 2     | Runtime adapter routable without restart   | 5    |
| 3     | Tool registration (`POST /tools`)          | 6    |
| 3     | `tools/list` per-user filtering            | 6    |
| 3     | `tools/call` authz denial → -32002         | 6    |
| 3     | `tools/call` happy path → forwarded        | 6    |
| Hard  | Name validation (path traversal)           | 7    |
| Hard  | Env prefix rejection                       | 7    |
| Hard  | Consistent 401 across planes               | 7    |
