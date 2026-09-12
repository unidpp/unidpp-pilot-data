#!/usr/bin/env bash
# seed-pilot.sh — seed the pilot registry and produce the demo artifacts.
#
# TODO.impl item C8. Idempotent and re-runnable against a running
# stack (./stack.sh start first):
#
#   [1] discovery seed   POST /admin/seed (C3 services, C4 protocol
#                        bindings, C5 verification mechanisms, C1
#                        units — the dataset /services serves)
#   [2] item seed        seed/items/*.json -> POST /items (the 10
#                        pilot items: 7 material-loop profiles, the
#                        EU machinery+battery and JP road-traffic
#                        jurisdiction profiles, the GB 4943.1 <->
#                        IEC 62368-1 equivalence transform)
#   [3] lens seed        seed/lens/*.json -> POST /profiles (the two
#                        projector-shaped lens profiles for the demo
#                        passport's EU/JP two-lens views)
#   [4] bindings         seed/bindings/*.json -> POST /applicability
#                        (dated bindings of EU + JP profiles onto the
#                        product type momiji:e8)
#   [5] demo passport    POST /passports + 5 typed events via the
#                        LIVE issuer, pack minted server-side
#   [6] CLI verify       unidpp verify of the pack against the
#                        issuer's public /keyring anchor
#   [7] archive          notarized Tier-C snapshot (anchored into the
#                        transparency log on 8392)
#   [8] projector        two-lens views of the demo passport
#                        (EU + JP) at the 2028-06-01 instant
#   [9] gateway          UNTP triad render (live issuer upstream) +
#                        EN 18222 REST render (fixture GTIN)
#  [10] as-of proof      the applicability queries (JP-only at
#                        2027-06-01, EU+JP at 2028-06-01)
#
# Every step writes a JSON artifact under demo/ (see README for the
# full list). Re-runs reuse the journaled state: item/profile/binding
# creations tolerate 409, the passport is issued once (409 -> the
# existing document is fetched), events are appended only up to the
# expected sequence.
set -euo pipefail

PILOT_DIR="$(cd "$(dirname "$0")" && pwd)"
FAMILY_DIR="${UNIDPP_FAMILY_DIR:-$(cd "$PILOT_DIR/.." && pwd)}"
DEMO_DIR="$PILOT_DIR/demo"
PASSPORTS_DIR="$PILOT_DIR/passports"
UNIDPP_CLI="${UNIDPP_BIN:-$FAMILY_DIR/unidpp-cli/target/release/unidpp}"

REGISTRY="http://127.0.0.1:8390"
TRUST="http://127.0.0.1:8391"
LOG="http://127.0.0.1:8392"
ISSUER="http://127.0.0.1:8393"
PROJECTOR="http://127.0.0.1:8394"
GATEWAY="http://127.0.0.1:8395"
ARCHIVE="http://127.0.0.1:8396"

# The demo passport: one Momiji E8 instance, both jurisdiction
# profiles in its config vector, fixed passport id (idempotent).
DEMO_ID="local:momiji:e8/J-000842"
DEMO_TYPE_REF="momiji:e8"
DEMO_URN="urn:unidpp:passport:pilot-e8-j000842"
DEMO_EO="urn:unidpp:actor:momiji-mobility"
EU_LENS="urn:unidpp:profile:pilot-eu-lens"
JP_LENS="urn:unidpp:profile:pilot-jp-lens"
EU_PROFILE="urn:unidpp:profile:eu-machinery-battery"
JP_PROFILE="urn:unidpp:profile:jp-road-traffic"
GTIN_FIXTURE="4006381333931"   # the gateway's seeded tyre fixture
VIEW_AT="2028-06-01T00:00:00Z" # EU + JP both in force for momiji:e8

mkdir -p "$DEMO_DIR" "$PASSPORTS_DIR"

die() { echo "seed-pilot.sh: $*" >&2; exit 1; }
say() { printf '  %s\n' "$*"; }

json_get() { # json_get <file> <python-expr over doc>
  python3 - "$1" "$2" <<'PYEOF'
import json, sys
doc = json.load(open(sys.argv[1]))
print(eval(sys.argv[2], {}, {"doc": doc}))
PYEOF
}

healthy() { curl -sf -m 2 "$1/healthz" >/dev/null 2>&1; }

# post_json <url> <body> <out-file> <ok-codes> — 4xx inside ok-codes
# is tolerated (idempotent re-runs), other failures abort.
post_json() {
  local url="$1" body_file="$2" out="$3" ok="$4" code
  code="$(curl -s -o "$out" -w '%{http_code}' -m 20 -X POST "$url" \
    -H 'content-type: application/json' --data-binary "@$body_file")"
  case ",$ok," in *",$code,"*) ;; *) die "POST $url -> $code: $(head -c 300 "$out")" ;; esac
  echo "$code"
}

# ---------------------------------------------------------------------------
# [0] Preconditions: the stack must be up.
# ---------------------------------------------------------------------------
for pair in "registry:$REGISTRY" "trust:$TRUST" "log:$LOG" "issuer:$ISSUER" \
            "projector:$PROJECTOR" "gateway:$GATEWAY" "archive:$ARCHIVE"; do
  name="${pair%%:*}"; url="${pair#*:}"
  healthy "$url" || die "$name is not healthy at $url — run ./stack.sh start first"
done
[[ -x "$UNIDPP_CLI" ]] || die "unidpp CLI missing at $UNIDPP_CLI (cd unidpp-cli && cargo build --release)"

echo "== [1] discovery seed (C3/C4/C5 + C1 units)"
units="$(curl -sf -m 5 "$REGISTRY/units" | python3 -c 'import json,sys; print(len(json.load(sys.stdin).get("items", [])))' 2>/dev/null || echo 0)"
if [[ "${units:-0}" == "0" ]]; then
  tmp="$DEMO_DIR/.seed-body"; printf '{}' > "$tmp"
  post_json "$REGISTRY/admin/seed" "$tmp" "$DEMO_DIR/00-discovery-seed.json" "200,201"
  rm -f "$tmp"
  say "seeded: $(json_get "$DEMO_DIR/00-discovery-seed.json" '" ".join(f"{k}={v}" for k,v in doc["counts"].items())')"
else
  say "already present ($units units — journal replay)"
fi

echo "== [2] item seed (10 pilot items)"
for body in "$PILOT_DIR"/seed/items/*.json; do
  code="$(post_json "$REGISTRY/items" "$body" "$DEMO_DIR/.resp" "201,409")"
  item="$(json_get "$body" 'doc["item_id"]')"
  if [[ "$code" == "201" ]]; then say "registered $item"; else say "present    $item"; fi
done

echo "== [3] lens seed (projector-shaped lens profiles)"
for body in "$PILOT_DIR"/seed/lens/*.json; do
  code="$(post_json "$REGISTRY/profiles" "$body" "$DEMO_DIR/.resp" "201,409")"
  item="$(json_get "$body" 'doc["item_id"]')"
  if [[ "$code" == "201" ]]; then say "registered $item"; else say "present    $item"; fi
done

echo "== [4] applicability bindings (dated, on product type momiji:e8)"
for body in "$PILOT_DIR"/seed/bindings/*.json; do
  profile="$(json_get "$body" 'doc["profile_id"]')"
  subject="$(json_get "$body" 'doc["product_type"]')"
  # The binding check must look into the future (both effective
  # windows start 2027/2028) — without `at` the query answers "now",
  # where neither duty is in force yet, and re-runs would duplicate
  # the binding (the registry deliberately does not dedupe).
  bound="$(curl -sf -m 5 "$REGISTRY/applicability?product_type=$subject&at=2030-01-01T00:00:00Z" \
    | python3 -c "import json,sys; doc=json.load(sys.stdin); print(any(b['binding']['profile_item']=='$profile' for b in doc['applicability']))")"
  if [[ "$bound" == "True" ]]; then
    say "present    $profile on $subject"
  else
    post_json "$REGISTRY/applicability" "$body" "$DEMO_DIR/.resp" "201"
    say "bound      $profile on $subject (from $(json_get "$body" 'doc["effective_from"]'))"
  fi
done

# ---------------------------------------------------------------------------
# [5] Demo passport via the LIVE issuer: create once, append events
# only up to the expected sequence (idempotent re-runs).
# ---------------------------------------------------------------------------
echo "== [5] demo passport (live issuer)"
create_body="$DEMO_DIR/.create.json"
python3 - "$DEMO_ID" "$DEMO_TYPE_REF" "$DEMO_URN" "$DEMO_EO" "$EU_PROFILE" "$JP_PROFILE" > "$create_body" <<'PYEOF'
import json, sys
identity, type_ref, urn, eo, eu, jp = sys.argv[1:7]
print(json.dumps({
    "identity": identity,
    "type_ref": type_ref,
    "capability": "S2",
    "eo_id": eo,
    "resolver_uri": "https://resolver.unidpp.org/r/pilot-e8-j000842",
    "passport_id": urn,
    "config": [eu, jp],
}))
PYEOF
code="$(post_json "$ISSUER/passports" "$create_body" "$DEMO_DIR/.resp" "201,409")"
if [[ "$code" == "201" ]]; then
  cp "$DEMO_DIR/.resp" "$DEMO_DIR/01-passport-created.json"
  say "issued $DEMO_URN"
else
  say "already issued $DEMO_URN (issuer journal replay)"
fi

# The events, in order: (type, payload-json, actor, role, at).
event_body() { # event_body <type> <payload> <actor> <role> <at>
  python3 - "$@" <<'PYEOF'
import json, sys
type_token, data_json, actor, role, at = sys.argv[1:6]
print(json.dumps({"type": type_token, "data": json.loads(data_json),
                  "actor": actor, "actor_role": role, "at": at}))
PYEOF
}
# The events, in order: (type, payload-json, actor, role, at). The
# milestone is stamped at seed time — the S2 BMS dumps its counter
# history on physical read (story beat B5), and a fresh stamp is what
# keeps the minted pack inside the Tier-A freshness window.
NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
DEMO_EVENTS=(
  'issuance|{"Issuance":{"derived":false,"inputs":[]}}|momiji-mobility|issuing authority|2026-07-14T08:00:00Z'
  'correction|{"Correction":{"field":"subject.markets","prior_value":"","new_value":"EU,JP","reason":"market placement declaration (EU, JP)"}}|momiji-mobility|economic operator|2026-07-14T09:00:00Z'
  'correction|{"Correction":{"field":"de.dpp.operator-id","prior_value":"","new_value":"urn:unidpp:actor:momiji-mobility","reason":"EU/JP DPP operator identifier"}}|momiji-mobility|economic operator|2026-07-14T09:05:00Z'
  'correction|{"Correction":{"field":"de.jp.type-approval-mark","prior_value":"","new_value":"jp-epac-type-2027","reason":"JP road-traffic EPAC type approval"}}|momiji-type-approval|issuing authority|2026-07-14T09:10:00Z'
  "milestone.record|{\"MilestoneRecord\":{\"counters\":{\"de.dpp.reparability-score\":\"8.1\",\"de.dpp.carbon-footprint\":\"96.4\",\"battery.capacity-kwh\":\"0.72\"}}}|e8-bms-controller|device|$NOW"
)
want_events="${#DEMO_EVENTS[@]}"
have="$(curl -sf -m 5 "$ISSUER/passports/$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$DEMO_URN")" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("events", 0))')"
i="$have"
while [[ "$i" -lt "$want_events" ]]; do
  spec="${DEMO_EVENTS[$i]}"
  type_token="${spec%%|*}"; rest="${spec#*|}"
  payload="${rest%%|*}"; rest="${rest#*|}"
  actor="${rest%%|*}"; rest="${rest#*|}"
  role="${rest%%|*}"; at="${rest#*|}"
  body="$DEMO_DIR/.event.json"
  event_body "$type_token" "$payload" "$actor" "$role" "$at" > "$body"
  enc_urn="$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$DEMO_URN")"
  post_json "$ISSUER/passports/$enc_urn/events" "$body" "$DEMO_DIR/.resp" "201"
  say "event $((i + 1))/$want_events: $type_token ($(json_get "$DEMO_DIR/.resp" 'doc.get("event_type", "?")'))"
  i=$((i + 1))
done
say "log: $want_events/$want_events events (as-of-reconstructable)"

# The full passport view: demo artifact + the projector's store copy.
enc_urn="$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$DEMO_URN")"
curl -sf -m 5 "$ISSUER/passports/$enc_urn" -o "$DEMO_DIR/02-passport-view.json" \
  || die "cannot fetch the passport view"
cp "$DEMO_DIR/02-passport-view.json" "$PASSPORTS_DIR/pilot-e8.json"
log_head="$(json_get "$DEMO_DIR/02-passport-view.json" 'doc["log_head"]')"
say "view: status $(json_get "$DEMO_DIR/02-passport-view.json" 'doc["status"]') · log_head ${log_head:0:16}…"

# ---------------------------------------------------------------------------
# [6] Tier-A pack + CLI verification against the issuer's keyring.
# ---------------------------------------------------------------------------
echo "== [6] Tier-A pack + CLI verify (anchor from the issuer /keyring)"
printf '{}' > "$DEMO_DIR/.packreq.json"
enc_urn="$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$DEMO_URN")"
post_json "$ISSUER/passports/$enc_urn/pack" "$DEMO_DIR/.packreq.json" "$DEMO_DIR/03-tier-a-pack.json" "201"
cp "$DEMO_DIR/03-tier-a-pack.json" "$DEMO_DIR/04-issuer-keyring.json.candidate" 2>/dev/null || true
curl -sf -m 5 "$ISSUER/keyring" -o "$DEMO_DIR/04-issuer-keyring.json"
json_get "$DEMO_DIR/03-tier-a-pack.json" 'doc["pack"]' > "$DEMO_DIR/pack.hex"
anchor="$(json_get "$DEMO_DIR/04-issuer-keyring.json" 'doc["roles"]["pack"]["public"]')"
say "pack: $(json_get "$DEMO_DIR/03-tier-a-pack.json" 'doc["bytes"]') bytes, QR v$(json_get "$DEMO_DIR/03-tier-a-pack.json" 'doc["qr_version"]'), anchor ${anchor:0:16}… (ECDSA-P256)"
rm -f "$DEMO_DIR/04-issuer-keyring.json.candidate"

# Verify under archival semantics: the demo passport's last event
# sets the pack's content as-of stamp, so the seed is idempotent
# across time — a pack minted on day one verifies PASS on day five
# hundred as a document (static semantics, never stale). Freshness
# semantics are exercised by the e2e demo's pinned as-of moments.
set +e
verify_out="$("$UNIDPP_CLI" verify "$DEMO_DIR/pack.hex" --anchor "$anchor" --max-age 0 --json 2>"$DEMO_DIR/.verify.stderr")"
verify_code=$?
set -e
python3 - "$verify_code" "$verify_out" > "$DEMO_DIR/05-cli-verify.json" <<'PYEOF'
import json, sys
code, out = sys.argv[1], sys.argv[2]
try:
    doc = json.loads(out)
except json.JSONDecodeError:
    doc = {"raw": out}
doc["exit_code"] = int(code)  # 0 pass / 1 degraded / 2 fail
print(json.dumps(doc, indent=1, sort_keys=True))
PYEOF
rm -f "$DEMO_DIR/.verify.stderr"
grade="$(json_get "$DEMO_DIR/05-cli-verify.json" 'doc.get("verdict", doc.get("grade", "?"))')"
[[ "$verify_code" -eq 0 ]] || die "CLI verify graded '$grade' (exit $verify_code), expected PASS (0)"
say "verified: PASS (exit 0) — the three readings named, coverage complete"

# ---------------------------------------------------------------------------
# [7] Tier-C: a notarized snapshot of the demo passport (anchored
# into the transparency log on 8392 by the archive itself).
# ---------------------------------------------------------------------------
echo "== [7] notarized Tier-C snapshot (anchored in the transparency log)"
state_hash="$(python3 - "$DEMO_DIR/02-passport-view.json" <<'PYEOF'
import hashlib, sys
print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())
PYEOF
)"
python3 - "$DEMO_URN" "$state_hash" "$log_head" > "$DEMO_DIR/.snapshot-req.json" <<'PYEOF'
import json, sys
print(json.dumps({
    "passport_id": sys.argv[1],
    "state_hash": sys.argv[2],
    "log_head": sys.argv[3],
    "submitter": "urn:unidpp:actor:pilot-operator",
    "state_size": 0,
}))
PYEOF
post_json "$ARCHIVE/snapshots" "$DEMO_DIR/.snapshot-req.json" "$DEMO_DIR/06-archive-snapshot.json" "201"
snap_status="$(json_get "$DEMO_DIR/06-archive-snapshot.json" 'doc["oais"]["provenance"]["anchoring"]["status"]')"
say "snapshot $(json_get "$DEMO_DIR/06-archive-snapshot.json" 'doc["snapshot_id"]') notarized, anchoring: $snap_status"

# ---------------------------------------------------------------------------
# [8] The projector: the same passport under the EU and the JP lens,
# at the instant both duties are in force.
# ---------------------------------------------------------------------------
echo "== [8] projector two-lens views (at $VIEW_AT)"
for lens in "$EU_LENS" "$JP_LENS"; do
  tag="eu"; [[ "$lens" == *jp* ]] && tag="jp"
  curl -sf -m 15 -G "$PROJECTOR/view" \
    --data-urlencode "passport=$DEMO_URN" \
    --data-urlencode "profile=$lens" \
    --data-urlencode "actor=importer" \
    --data-urlencode "at=$VIEW_AT" \
    -o "$DEMO_DIR/07-projector-view-$tag.json" \
    || die "projector view failed for $lens ($(head -c 300 "$DEMO_DIR/07-projector-view-$tag.json" 2>/dev/null))"
  say "$tag view: source $(json_get "$DEMO_DIR/07-projector-view-$tag.json" 'doc["profile"]["source"]'), coverage $(json_get "$DEMO_DIR/07-projector-view-$tag.json" 'doc["coverage"]["elements_present"]')/$(json_get "$DEMO_DIR/07-projector-view-$tag.json" 'doc["coverage"]["elements_required"]') ($(json_get "$DEMO_DIR/07-projector-view-$tag.json" 'doc["coverage"]["complete"]'))"
done

# ---------------------------------------------------------------------------
# [9] The gateway: UNTP triad (live issuer upstream) + EN 18222 render.
# ---------------------------------------------------------------------------
echo "== [9] gateway renders (UNTP triad + EN 18222)"
enc_urn="$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$DEMO_URN")"
curl -sf -m 15 -G "$GATEWAY/untp/product/$enc_urn" -o "$DEMO_DIR/08-gateway-untp.json" \
  || die "UNTP render failed"
say "UNTP triad: source $(json_get "$DEMO_DIR/08-gateway-untp.json" 'doc["rendering"]["source"]'), outcome $(json_get "$DEMO_DIR/08-gateway-untp.json" 'doc["verdict"]["outcome"]') ($(json_get "$DEMO_DIR/08-gateway-untp.json" 'doc["verdict"]["coverage"]["signatures"]["verified"]') signatures verified)"
curl -sf -m 15 "$GATEWAY/en18222/v1/dppsByProductId/$GTIN_FIXTURE?representation=full" \
  -o "$DEMO_DIR/09-gateway-en18222.json" || die "EN 18222 render failed"
say "EN 18222 render: dpp $(json_get "$DEMO_DIR/09-gateway-en18222.json" 'doc["digitalProductPassportId"]') via GTIN $GTIN_FIXTURE"

# ---------------------------------------------------------------------------
# [10] The as-of applicability proof (the pilot's headline query).
# ---------------------------------------------------------------------------
echo "== [10] as-of applicability for momiji:e8"
curl -sf -m 5 "$REGISTRY/applicability?product_type=momiji:e8&at=2027-06-01T00:00:00Z" \
  -o "$DEMO_DIR/10-applicability-2027-06-01.json" || die "as-of query failed (2027)"
curl -sf -m 5 "$REGISTRY/applicability?product_type=momiji:e8&at=2028-06-01T00:00:00Z" \
  -o "$DEMO_DIR/11-applicability-2028-06-01.json" || die "as-of query failed (2028)"
at_2027="$(json_get "$DEMO_DIR/10-applicability-2027-06-01.json" 'sorted({b["binding"]["profile_item"] for b in doc["applicability"]})')"
at_2028="$(json_get "$DEMO_DIR/11-applicability-2028-06-01.json" 'sorted({b["binding"]["profile_item"] for b in doc["applicability"]})')"
say "at 2027-06-01: $at_2027"
say "at 2028-06-01: $at_2028"

# Tidy the scratch files (the .resp/.event/.create bodies are not artifacts).
rm -f "$DEMO_DIR"/.resp "$DEMO_DIR"/.event.json "$DEMO_DIR"/.create.json \
      "$DEMO_DIR"/.packreq.json "$DEMO_DIR"/.snapshot-req.json "$DEMO_DIR"/.seed-body

echo
echo "== demo artifacts:"
(cd "$DEMO_DIR" && ls -1 *.json *.hex 2>/dev/null | sed 's/^/   demo\//')
echo "== seed-pilot complete — the stack stays up (./stack.sh status, ./stack.sh stop)"
