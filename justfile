# Convenience recipes for testing mcp-oxide. Run `just --list` for the menu.
# Requires: cargo, docker (for smoke-*), jq + openssl (for smoke-token).

_default:
    @just --list

# Fast unit+integration tests across the workspace.
test:
    cargo test --all-targets

# Full suite including the docker feature for the deployment provider.
test-full:
    cargo test --all-targets --features docker

# Clippy gate (pedantic, deny warnings).
lint:
    cargo clippy --all-targets --features docker -- -D warnings
    cargo clippy --all-targets --no-default-features -- -D warnings

# Start the smoke-test docker-compose stack.
smoke-up:
    docker compose -f deploy/smoke/docker-compose.yaml up --build -d
    @echo "Gateway on :8080, mocks on :18090/18091/18092."

# Tail logs for the smoke stack.
smoke-logs:
    docker compose -f deploy/smoke/docker-compose.yaml logs -f

# Tear the smoke stack down (incl. volumes).
smoke-down:
    docker compose -f deploy/smoke/docker-compose.yaml down -v

# Print an HS256 token using the smoke-stack secret.
# Usage: just smoke-token admin mcp.admin
smoke-token sub roles:
    #!/usr/bin/env bash
    set -euo pipefail
    secret=$(tr -d '\n' < deploy/smoke/jwt.key)
    now=$(date +%s)
    roles_json=$(jq -nc --arg r "{{roles}}" '($r | split(","))')
    payload=$(jq -nc \
      --arg sub "{{sub}}" --arg iss "mcp-oxide-smoke" --arg aud "mcp-oxide" \
      --argjson now "$now" --argjson roles "$roles_json" \
      '{sub:$sub, iss:$iss, aud:$aud, iat:$now, exp:($now+3600), roles:$roles}')
    header='{"alg":"HS256","typ":"JWT"}'
    b64(){ openssl base64 -e -A | tr '+/' '-_' | tr -d '='; }
    h=$(printf %s "$header"  | b64)
    p=$(printf %s "$payload" | b64)
    sig=$(printf %s "$h.$p" | openssl dgst -sha256 -hmac "$secret" -binary | b64)
    printf '%s.%s.%s\n' "$h" "$p" "$sig"

# Run the curl playbook against the live smoke stack.
# Requires `just smoke-up` first.
smoke-playbook:
    @echo "See tests/smoke/README.md for the full walkthrough."
    @echo "Run the following to reproduce the key assertions:"
    @echo ""
    @echo "  ADMIN=\$(just smoke-token admin  mcp.admin)"
    @echo "  VIEWER=\$(just smoke-token viewer mcp.viewer)"
    @echo "  curl -s http://localhost:8080/healthz | jq"
