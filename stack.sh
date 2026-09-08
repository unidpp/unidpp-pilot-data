#!/usr/bin/env bash
# stack.sh — the full UniDPP pilot stack on 127.0.0.1:8390-8397.
#
# TODO.impl item C8 (pilot-orchestration). Builds (cargo build
# --release where needed) and starts every service of the pilot as
# one dependency-ordered unit, then waits for each /healthz:
#
#   8390  unidpp-registry   the ISO 19135 item + discovery registry
#                           (journal: registry-journal.jsonl at the
#                           repo root — the seeded 10 items + 2
#                           applicability bindings replay on start)
#   8391  unidpp-trust      the SIGNATIF trust-graph service
#   8392  unidpp-log        the transparency-log anchor service
#   8393  unidpp-issuer     the passport lifecycle issuer
#                           (UNIDPP_ISSUER_REGISTRY_URL -> 8390)
#   8394  unidpp-projector  the lens projection service
#                           (UNIDPP_REGISTRY_URL -> 8390,
#                            passports from ./passports/)
#   8395  unidpp-gateway    the interop gateway: UNTP + EN 18222
#                           renders (UNIDPP_ISSUER_URL -> 8393)
#   8396  unidpp-archive    the Tier-C notarized snapshot service
#                           (UNIDPP_LOG_URL -> 8392)
#   8397  (reserved)
#   8399  unidpp-registry (JP)  the JP national peer node: the same
#                           registry binary, its own journal and its
#                           own profile items (jurisdiction JP) —
#                           registry-jp.unidpp.org via jp-tunnel.token.
#                           National registries are peers, not children
#                           (the ePassport doctrine).
#
# Plus the existing pilot tunnel: when tunnel.token is present a
# cloudflared named tunnel is (re)started for pilot.unidpp.org ->
# 127.0.0.1:8390; a tunnel that is already running is adopted, not
# duplicated.
#
# Usage:
#   ./stack.sh start     # build-if-needed + start + wait healthy
#   ./stack.sh stop      # stop everything (journals are preserved)
#   ./stack.sh status    # per-service health + tunnel + journal state
#
# State: ./run/ holds pid files, per-service logs and the journals of
# the services that persist one (the registry journal stays at the
# repo root — it predates this script and is the seed of record).
# ./passports/ is the projector's passport store (seed-pilot.sh
# writes the demo passport document there). ./demo/ holds the demo
# artifacts seed-pilot.sh produces.
#
# All services run in dev mode (no admin tokens, seeded-dev
# keyrings): a demonstration pilot, not a production deployment.
set -euo pipefail

PILOT_DIR="$(cd "$(dirname "$0")" && pwd)"
RUN_DIR="$PILOT_DIR/run"
PASSPORTS_DIR="$PILOT_DIR/passports"
DEMO_DIR="$PILOT_DIR/demo"
REGISTRY_JOURNAL="$PILOT_DIR/registry-journal.jsonl"
JP_REGISTRY_JOURNAL="$PILOT_DIR/jp-registry-journal.jsonl"
mkdir -p "$RUN_DIR" "$PASSPORTS_DIR"

# Family layout: the service repos sit beside this directory inside
# the unidpp workspace (override with UNIDPP_FAMILY_DIR).
FAMILY_DIR="${UNIDPP_FAMILY_DIR:-$(cd "$PILOT_DIR/.." && pwd)}"

UNIDPP_CLI="$FAMILY_DIR/unidpp-cli/target/release/unidpp"

die() { echo "stack.sh: $*" >&2; exit 1; }

# service <name> <port> <repo> — resolve a service's binary path.
svc_bin() { # svc_bin <repo>
  echo "$FAMILY_DIR/$1/target/release/$1"
}

healthz() { # healthz <port>
  curl -sf -m 2 "http://127.0.0.1:$1/healthz" >/dev/null 2>&1
}

# The discovery document's `service` field names the listener — a
# healthy /healthz alone cannot tell two family services apart (they
# all answer "ok"), which matters on this shared box: the
# unidpp-registry-kit launcher also defaults to 8391.
service_id() { # service_id <port>
  curl -sf -m 2 "http://127.0.0.1:$1/" 2>/dev/null \
    | jq -r '.service // empty' 2>/dev/null
}

is_ours() { # is_ours <port> <expected-service-name>
  healthz "$1" && [[ "$(service_id "$1")" == "$2" ]]
}

port_taken() { # port_taken <port>
  lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
}

# ---------------------------------------------------------------------------
# Build: cargo build --release per service, only when the release
# binary is missing (UNIDPP_FORCE_BUILD=1 rebuilds unconditionally).
# ---------------------------------------------------------------------------
ensure_bin() { # ensure_bin <repo>
  local repo="$1" bin
  bin="$(svc_bin "$repo")"
  if [[ -x "$bin" && "${UNIDPP_FORCE_BUILD:-0}" != "1" ]]; then return 0; fi
  command -v cargo >/dev/null 2>&1 \
    || die "cargo not found and no prebuilt binary at $bin"
  echo "==> building $repo (release)"
  (cd "$FAMILY_DIR/$repo" && cargo build --release)
  [[ -x "$bin" ]] || die "build finished but $bin is missing"
}

# ---------------------------------------------------------------------------
# Start one service: reuse when healthy AND the right service, refuse
# foreign listeners, nohup with env wiring, pid + log under run/.
# start_service <name> <repo> <port> <health-timeout-secs> [env VAR=V ...]
# ---------------------------------------------------------------------------
start_service() {
  local name="$1" repo="$2" port="$3" tries="${4:-60}"; shift 4
  local bin url pidfile logfile
  bin="$(svc_bin "$repo")"
  url="http://127.0.0.1:$port"
  pidfile="$RUN_DIR/$name.pid"
  logfile="$RUN_DIR/$name.log"

  if is_ours "$port" "$repo"; then
    echo "==> $name already healthy on $url (reusing)"
    return 0
  fi
  if port_taken "$port"; then
    local holder
    holder="$(service_id "$port")"
    if [[ -n "$holder" ]]; then
      die "port $port is held by '$holder' but '$repo' needs it. If it is the unidpp-registry-kit demo registry (its launcher defaults to 8391), stop it with 'unidpp-registry-kit/bin/run-registry.sh stop' — or restart it on another port with KIT_PORT=8399."
    fi
    die "port $port is held by a non-healthy listener — stop it first (lsof -nP -iTCP:$port)"
  fi

  ensure_bin "$repo"
  echo "==> starting $name on $url"
  nohup env "$@" "$bin" >> "$logfile" 2>&1 &
  echo $! > "$pidfile"

  local i
  for i in $(seq 1 "$tries"); do
    is_ours "$port" "$repo" && return 0
    kill -0 "$(cat "$pidfile")" 2>/dev/null \
      || die "$name exited during startup (see $logfile)"
    sleep 0.5
  done
  die "$name did not become healthy on $url within $((tries / 2))s (see $logfile)"
}

# ---------------------------------------------------------------------------
# The tunnel: adopt a running cloudflared (matching tunnel.token) or
# start one. Ingress is pilot.unidpp.org -> 127.0.0.1:8390.
# ---------------------------------------------------------------------------
ensure_tunnel() {
  local pidfile="$RUN_DIR/tunnel.pid" logfile="$RUN_DIR/tunnel.log"
  if [[ ! -s "$PILOT_DIR/tunnel.token" ]]; then
    echo "==> no tunnel.token — skipping the public tunnel"
    return 0
  fi
  # Adopt any cloudflared already running with this token (it may
  # have been started by hand before stack.sh existed).
  local adopted
  adopted="$(pgrep -f "cloudflared tunnel run --token" | head -1 || true)"
  if [[ -n "$adopted" ]]; then
    echo "$adopted" > "$pidfile"
    echo "==> tunnel: adopted running cloudflared (pid $adopted; pilot.unidpp.org -> 8390)"
    return 0
  fi
  command -v cloudflared >/dev/null 2>&1 \
    || { echo "==> tunnel.token present but cloudflared is not installed — skipping"; return 0; }
  echo "==> starting cloudflared named tunnel (pilot.unidpp.org -> 8390)"
  nohup cloudflared tunnel run --token "$(cat "$PILOT_DIR/tunnel.token")" \
    --url http://127.0.0.1:8390 > "$logfile" 2>&1 &
  echo $! > "$pidfile"
}

ensure_jp_tunnel() {
  local pidfile="$RUN_DIR/jp-tunnel.pid" logfile="$RUN_DIR/jp-tunnel.log"
  if [[ ! -s "$PILOT_DIR/jp-tunnel.token" ]]; then
    echo "==> no jp-tunnel.token — skipping the JP node's public tunnel"
    return 0
  fi
  command -v cloudflared >/dev/null 2>&1 \
    || { echo "==> jp-tunnel.token present but cloudflared is not installed — skipping"; return 0; }
  if [[ -f "$pidfile" ]] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
    echo "==> jp-tunnel: already running (pid $(cat "$pidfile"))"
    return 0
  fi
  echo "==> starting cloudflared named tunnel (registry-jp.unidpp.org -> 8399)"
  nohup cloudflared tunnel run --token "$(cat "$PILOT_DIR/jp-tunnel.token")" \
    --url http://127.0.0.1:8399 > "$logfile" 2>&1 &
  echo $! > "$pidfile"
}

# ---------------------------------------------------------------------------
cmd_start() {
  # Registry first: the issuer forwards to it, the projector reads
  # profile items and units from it, the gateway verdicts anchor
  # against the issuer. Journals replay on start; nothing is wiped.
  start_service registry unidpp-registry 8390 60 \
    UNIDPP_REGISTRY_BIND=127.0.0.1:8390 \
    UNIDPP_REGISTRY_STATE_FILE="$REGISTRY_JOURNAL"

  start_service trust unidpp-trust 8391 60 \
    UNIDPP_TRUST_BIND=127.0.0.1:8391 \
    UNIDPP_TRUST_STATE_FILE="$RUN_DIR/trust-journal.jsonl"

  start_service log unidpp-log 8392 60 \
    UNIDPP_LOG_BIND=127.0.0.1:8392 \
    UNIDPP_LOG_ID=unidpp-pilot-log-1 \
    UNIDPP_LOG_STATE_FILE="$RUN_DIR/log-journal.jsonl" \
    UNIDPP_LOG_EXTERNAL_TSA_URL="${UNIDPP_LOG_EXTERNAL_TSA_URL:-http://timestamp.digicert.com}"

  start_service issuer unidpp-issuer 8393 60 \
    UNIDPP_ISSUER_BIND=127.0.0.1:8393 \
    UNIDPP_ISSUER_STATE_FILE="$RUN_DIR/issuer-journal.jsonl" \
    UNIDPP_ISSUER_REGISTRY_URL=http://127.0.0.1:8390

  start_service projector unidpp-projector 8394 60 \
    UNIDPP_PROJECTOR_BIND=127.0.0.1:8394 \
    UNIDPP_REGISTRY_URL=http://127.0.0.1:8390 \
    UNIDPP_PROJECTOR_PASSPORTS_DIR="$PASSPORTS_DIR"

  start_service gateway unidpp-gateway 8395 60 \
    UNIDPP_GATEWAY_BIND=127.0.0.1:8395 \
    UNIDPP_ISSUER_URL=http://127.0.0.1:8393

  start_service archive unidpp-archive 8396 60 \
    UNIDPP_ARCHIVE_BIND=127.0.0.1:8396 \
    UNIDPP_ARCHIVE_STATE_FILE="$RUN_DIR/archive-journal.jsonl" \
    UNIDPP_ARCHIVE_SNAPSHOT_DIR="$RUN_DIR/archive-snapshots" \
    UNIDPP_LOG_URL=http://127.0.0.1:8392

  # The admin console: the operator manifest as its configuration.
  start_service console unidpp-console 8389 60 \
    UNIDPP_CONSOLE_BIND=127.0.0.1:8389 \
    UNIDPP_CONSOLE_MANIFEST="$PILOT_DIR/unidpp-operator.yaml"

  # The JP national peer node: the same binary, its own journal — a
  # second jurisdiction on the same machine (the deployment shape the
  # registry starter kit demonstrates).
  start_service jp-registry unidpp-registry 8399 60 \
    UNIDPP_REGISTRY_BIND=127.0.0.1:8399 \
    UNIDPP_REGISTRY_STATE_FILE="$JP_REGISTRY_JOURNAL"

  ensure_tunnel
  ensure_jp_tunnel

  echo
  echo "==> stack up: 8390 registry · 8391 trust · 8392 log · 8393 issuer"
  echo "               8394 projector · 8395 gateway · 8396 archive"
  echo "==> next: ./seed-pilot.sh   (idempotent: seed + demo artifacts)"
}

# ---------------------------------------------------------------------------
cmd_stop() {
  local name pid
  for name in console archive gateway projector issuer log trust registry jp-registry tunnel jp-tunnel; do
    local pidfile="$RUN_DIR/$name.pid"
    if [[ -f "$pidfile" ]]; then
      pid="$(cat "$pidfile")"
      if kill "$pid" 2>/dev/null; then
        echo "==> stopped $name (pid $pid)"
      else
        echo "==> $name pid $pid not running (stale pid file)"
      fi
      rm -f "$pidfile"
    fi
  done
  echo "==> journals preserved (registry-journal.jsonl, run/*-journal.jsonl)"
}

# ---------------------------------------------------------------------------
cmd_status() {
  local failed=0
  for spec in \
    "registry:unidpp-registry:8390" \
    "trust:unidpp-trust:8391" \
    "log:unidpp-log:8392" \
    "issuer:unidpp-issuer:8393" \
    "projector:unidpp-projector:8394" \
    "gateway:unidpp-gateway:8395" \
    "archive:unidpp-archive:8396"; do
    local name rest repo port
    name="${spec%%:*}"; rest="${spec#*:}"; repo="${rest%%:*}"; port="${rest##*:}"
    if is_ours "$port" "$repo"; then
      printf '  %-10s %-18s http://127.0.0.1:%s  healthy\n' "$name" "($repo)" "$port"
    elif port_taken "$port"; then
      printf '  %-10s %-18s http://127.0.0.1:%s  HELD BY %s\n' "$name" "($repo)" "$port" "$(service_id "$port" || echo '?')"
      failed=1
    else
      printf '  %-10s %-18s http://127.0.0.1:%s  DOWN\n' "$name" "($repo)" "$port"
      failed=1
    fi
  done

  # Registry content: items + discovery records replayed from the journal.
  if healthz 8390; then
    local items services
    items="$(curl -sf -m 3 http://127.0.0.1:8390/items | jq -r '.items | length' 2>/dev/null || echo '?')"
    services="$(curl -sf -m 3 http://127.0.0.1:8390/services | jq -r '.services | length' 2>/dev/null || echo 0)"
    echo "  registry journal: $(wc -l < "$REGISTRY_JOURNAL" 2>/dev/null | tr -d ' ') records -> $items items, $services discovery services"
    echo "  applicability (momiji:e8 @2028-06-01): $(curl -sf -m 3 'http://127.0.0.1:8390/applicability?product_type=momiji:e8&at=2028-06-01T00:00:00Z' | jq -r '[.applicability[].binding.profile_item] | join(", ")' 2>/dev/null || echo '?')"
  fi
  if healthz 8392; then
    echo "  log tree head: $(curl -sf -m 3 http://127.0.0.1:8392/tree/head | jq -r '"size \(.tree_size) · root \(.root[0:16])…"' 2>/dev/null || echo '?')"
  fi
  if [[ -f "$RUN_DIR/tunnel.pid" ]] && kill -0 "$(cat "$RUN_DIR/tunnel.pid")" 2>/dev/null; then
    echo "  tunnel: running (pid $(cat "$RUN_DIR/tunnel.pid")) -> registry.unidpp.org"
  fi
  if [[ -f "$RUN_DIR/jp-tunnel.pid" ]] && kill -0 "$(cat "$RUN_DIR/jp-tunnel.pid")" 2>/dev/null; then
    echo "  jp-tunnel: running (pid $(cat "$RUN_DIR/jp-tunnel.pid")) -> registry-jp.unidpp.org"
  else
    echo "  tunnel: not running"
  fi
  [[ -d "$DEMO_DIR" ]] && echo "  demo artifacts: $(ls "$DEMO_DIR" 2>/dev/null | wc -l | tr -d ' ') files under demo/"
  exit "$failed"
}

case "${1:-start}" in
  start)  cmd_start ;;
  stop)   cmd_stop ;;
  status) cmd_status ;;
  *) die "usage: stack.sh start | stop | status" ;;
esac
