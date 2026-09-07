#!/usr/bin/env bash
# seed-jp.sh — seed the JP national peer node (127.0.0.1:8399).
#
# The peer doctrine (PLAN L6, the ePassport precedent): national
# registries are peers, not children. This node holds the JP
# jurisdiction's own data — its road-traffic profile and the binding
# of that profile onto a product type — in its own journal, served
# from its own register. The global registry keeps what is global
# (scheme namespaces, cross-register mappings); jurisdiction-shaped
# data lives with the jurisdiction.
#
# Idempotent and re-runnable against a running stack
# (./stack.sh start first; the JP node must be healthy on 8399).

set -euo pipefail

PILOT_DIR="$(cd "$(dirname "$0")" && pwd)"
JP_URL="${UNIDPP_JP_REGISTRY_URL:-http://127.0.0.1:8399}"

die() { echo "seed-jp.sh: $*" >&2; exit 1; }

curl -sf "$JP_URL/healthz" >/dev/null || die "JP node not healthy at $JP_URL (run: ./stack.sh start)"

post() { # post <path> <json-file> <label>
  code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "$JP_URL$1" \
    -H 'content-type: application/json' --data-binary @"$2")"
  case "$code" in
    201) echo "  [ok]   $3 registered" ;;
    409) echo "  [skip] $3 already present" ;;
    *)   die "$3: POST $1 returned $code" ;;
  esac
}

echo "seeding the JP national peer node ($JP_URL)"

# The JP road-traffic profile: this node's copy, this node's journal.
post /items "$PILOT_DIR/seed/items/profile-jp-road-traffic.json" \
  "jp-road-traffic profile"

# Its binding onto the product type — JP data, decided in JP.
post /applicability "$PILOT_DIR/seed/bindings/profile-jp-road-traffic-on-momiji-e8.json" \
  "jp-road-traffic binding"

echo "JP node seeded: 1 profile + 1 applicability binding (its own journal, its own register)"
