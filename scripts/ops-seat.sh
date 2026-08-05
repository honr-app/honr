#!/usr/bin/env bash
# Thin face over Board /api/ops-session + OpenShell connect.
# Does not store lifecycle: environment and conversation live on the Board.
set -euo pipefail

HONR_URL="${HONR_URL:-http://127.0.0.1:8080}"
COOKIE_JAR="${HONR_COOKIE_JAR:-${TMPDIR:-/tmp}/honr-ops.cookies}"
USER="${HONR_USER:-}"
PASS="${HONR_PASSWORD:-}"

usage() {
  cat <<'EOF'
Usage: ops-seat.sh <start|status|attach|park|resume|stop|login>

Board owns ops-session lifecycle. This script only calls REST and
`openshell sandbox connect` — no local session file beyond the auth cookie jar.

Env:
  HONR_URL          default http://127.0.0.1:8080
  HONR_COOKIE_JAR   default $TMPDIR/honr-ops.cookies
  HONR_USER / HONR_PASSWORD   for login (or run `login` interactively)
EOF
}

need_jq() {
  command -v jq >/dev/null || {
    echo "jq is required" >&2
    exit 1
  }
}

api() {
  local method=$1 path=$2
  shift 2
  curl -sS -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
    -X "$method" \
    -H 'Content-Type: application/json' \
    "$@" \
    "${HONR_URL}${path}"
}

login() {
  local u=${USER:-} p=${PASS:-}
  if [[ -z "$u" ]]; then
    read -r -p "honr username: " u
  fi
  if [[ -z "$p" ]]; then
    read -r -s -p "honr password: " p
    echo >&2
  fi
  api POST /auth/login -d "$(jq -nc --arg u "$u" --arg p "$p" '{username:$u,password:$p}')" >/dev/null
  echo "logged in → cookie jar $COOKIE_JAR" >&2
}

ensure_auth() {
  local code
  code=$(curl -sS -o /dev/null -w '%{http_code}' -c "$COOKIE_JAR" -b "$COOKIE_JAR" \
    "${HONR_URL}/api/ops-session" || true)
  if [[ "$code" == "401" ]]; then
    if [[ -n "${USER:-}" && -n "${PASS:-}" ]]; then
      login
    else
      echo "not authenticated (HTTP $code). Set HONR_USER/HONR_PASSWORD or run: $0 login" >&2
      exit 1
    fi
  fi
}

session_json() {
  api GET /api/ops-session
}

environment() {
  session_json | jq -r '.session.environment // empty'
}

cmd=${1:-}
case "$cmd" in
  login)
    need_jq
    login
    ;;
  start)
    need_jq
    ensure_auth
    api POST /api/ops-session -d '{}'
    echo >&2
    echo "Board session created; supervisor materializes the ops sandbox." >&2
    echo "Poll: $0 status" >&2
    ;;
  status)
    need_jq
    ensure_auth
    session_json | jq .
    ;;
  attach)
    need_jq
    ensure_auth
    env_name=$(environment)
    if [[ -z "$env_name" ]]; then
      echo "no session.environment yet — start the seat and wait for the supervisor" >&2
      session_json | jq . >&2
      exit 1
    fi
    echo "connecting to $env_name (Board still owns lifecycle)" >&2
    exec openshell sandbox connect "$env_name"
    ;;
  park)
    need_jq
    ensure_auth
    api POST /api/ops-session/park -d ''
    echo >&2
    ;;
  resume)
    need_jq
    ensure_auth
    api POST /api/ops-session/resume -d ''
    echo >&2
    ;;
  stop)
    need_jq
    ensure_auth
    api DELETE /api/ops-session
    echo "ops session cleared (supervisor stops agent + deletes sandbox)" >&2
    ;;
  ""|-h|--help|help)
    usage
    ;;
  *)
    echo "unknown command: $cmd" >&2
    usage >&2
    exit 1
    ;;
esac
