#!/usr/bin/env bash
# tenants/up.sh — bring up one tenant from its operator manifest.
#
#   ./tenants/up.sh acme        # the whitelabel EU tenant
#   ./tenants/up.sh acme-cn     # the sovereign CN tenant
#   ./tenants/up.sh acme stop
#
# A tenant is a directory: unidpp-operator.yaml + journals. Services
# start from the manifest's rendered environment (unidpp-config
# render-env) — zero per-tenant code, exactly as the whitelabel
# doctrine requires.
set -euo pipefail

PILOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
FAMILY_DIR="${UNIDPP_FAMILY_DIR:-$(cd "$PILOT_DIR/.." && pwd)}"
CONFIG_CLI="$FAMILY_DIR/unidpp-config/target/release/unidpp-config"
[ -x "$CONFIG_CLI" ] || CONFIG_CLI="$FAMILY_DIR/unidpp-config/target/debug/unidpp-config"
TENANT="${1:?usage: tenants/up.sh <name> [start|stop|status]}"
ACTION="${2:-start}"
MANIFEST="$PILOT_DIR/tenants/$TENANT/unidpp-operator.yaml"
RUN_DIR="$PILOT_DIR/run/tenants/$TENANT"
mkdir -p "$RUN_DIR"

tenant_env() { "$CONFIG_CLI" render-env "$1" "$MANIFEST"; }

svc() { # svc <name> <binary> — start with the manifest's env
  local name="$1" binary="$2"
  local pidfile="$RUN_DIR/$name.pid"
  if [ "$ACTION" = stop ]; then
    [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null && echo "stopped $TENANT/$name" || true
    return 0
  fi
  if [ "$ACTION" = status ]; then
    if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
      echo "  $name: running (pid $(cat "$pidfile"))"
    else
      echo "  $name: down"
    fi
    return 0
  fi
  [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null && { echo "  $name: already running"; return 0; }
  env $(tenant_env "$name") nohup "$FAMILY_DIR/$binary/target/release/unidpp-$name" \
    > "$RUN_DIR/$name.log" 2>&1 &
  echo $! > "$pidfile"
  echo "  $name: started (pid $(cat "$pidfile"))"
}

case "$ACTION" in
  start) echo "tenant $TENANT:" ;;
  stop)  echo "tenant $TENANT (stop):" ;;
  status) echo "tenant $TENANT (status):" ;;
  *) echo "unknown action $ACTION" >&2; exit 2 ;;
esac

[ -f "$MANIFEST" ] || { echo "no manifest at $MANIFEST" >&2; exit 1; }

# The console needs its manifest path in the environment.
env_extra() { [ "$1" = console ] && echo "UNIDPP_CONSOLE_MANIFEST=$MANIFEST" || true; }
svc_console() {
  local pidfile="$RUN_DIR/console.pid"
  if [ "$ACTION" = stop ] || [ "$ACTION" = status ]; then svc console anything; return; fi
  [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null && { echo "  console: already running"; return 0; }
  env $(tenant_env console) UNIDPP_CONSOLE_MANIFEST="$MANIFEST" \
    nohup "$FAMILY_DIR/unidpp-console/target/release/unidpp-console" \
    > "$RUN_DIR/console.log" 2>&1 &
  echo $! > "$pidfile"
  echo "  console: started (pid $(cat "$pidfile"))"
}

svc registry unidpp-registry
svc issuer unidpp-issuer
svc_console
