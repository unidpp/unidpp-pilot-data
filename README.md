# UniDPP Registry Pilot

The running UniDPP pilot: the full service stack on
`127.0.0.1:8390-8397`, the seeded registry (jurisdiction +
material-loop profiles), and the demo artifacts a re-run of
`unidpp-stack seed` produces against the live services.

## Stack (TODO.impl C8 — pilot-orchestration)

```sh
./ops/target/release/unidpp-stack up     # build-if-needed + start all ten + wait healthy
./ops/target/release/unidpp-stack seed   # idempotent: seed + demo artifacts (below)
./ops/target/release/unidpp-stack status # per-service health + journal + tunnel state
./ops/target/release/unidpp-stack down   # stop everything; journals are preserved
```

| Service | Port | Role | Health | Upstream |
|---|---|---|---|---|
| [unidpp-registry](../unidpp-registry) | 8390 | ISO 19135 item + discovery registry (C3/C4/C5, units); journal: `registry-journal.jsonl` | `GET /healthz` | — |
| [unidpp-trust](../unidpp-trust) | 8391 | SIGNATIF trust graph, keyring, revocations | `GET /healthz` | — |
| [unidpp-log](../unidpp-log) | 8392 | transparency-log anchor service (RFC 6962 receipts) | `GET /healthz` | — |
| [unidpp-issuer](../unidpp-issuer) | 8393 | passport lifecycle issuer (create/events/pack/verdict) | `GET /healthz` | registry 8390 |
| [unidpp-projector](../unidpp-projector) | 8394 | lens projection: a passport under a registered profile | `GET /healthz` | registry 8390, `passports/` |
| [unidpp-gateway](../unidpp-gateway) | 8395 | interop renders: UNTP VC triad + EN 18222 REST | `GET /healthz` | issuer 8393 |
| [unidpp-archive](../unidpp-archive) | 8396 | Tier-C notarized snapshots (OAIS, log-anchored) | `GET /healthz` | log 8392 |
| (reserved) | 8397 | — | — | — |

Notes:

- `docker-compose.yml` sketches the same stack containerized
  (build contexts into the sibling repos) — documented, not the
  primary path; `unidpp-stack` is.
- The launcher verifies each port serves the **right** service
  (discovery `service` field), because the unidpp-registry-kit
  launcher also defaults to 8391. If the kit's demo registry holds
  the port, stop it (`unidpp-registry-kit/bin/run-registry.sh stop`)
  or restart it with `KIT_PORT=8399`.
- Everything runs in dev mode (no admin tokens, seeded-dev keyrings)
  — a demonstration pilot, not a production deployment.
- State: `registry-journal.jsonl` (seed of record, at the repo root)
  and `run/*-journal.jsonl` replay on start; `passports/` is the
  projector's document store; `demo/` holds the artifacts.

## Seeded registry

- **10 items** (seed corpus in `seed/items/`, durable in the journal):
  7 material-loop profiles (extraction, 3TG, CRMA, end-of-waste, SRM,
  waste-shipment, reuse), the EU machinery+battery and JP
  road-traffic jurisdiction profiles (effective 2028-02-01 /
  2027-04-01), and the GB 4943.1 ↔ IEC 62368-1 equivalence transform.
- **2 applicability bindings** on product type `momiji:e8`
  (`seed/bindings/`) — the dated-binding demo: **as-of 2027-06-01 →
  [JP]; as-of 2028-06-01 → [EU, JP]**.
- **2 projector lens profiles** (`seed/lens/`) for the demo
  passport's EU/JP two-lens views.
- **Discovery dataset** (`POST /admin/seed`): 8 C3 services, 5 C4
  protocol bindings, 3 C5 verification mechanisms, 10 C1 units.

## Demo artifacts (`unidpp-stack seed`, all under `demo/`)

| Artifact | What it is |
|---|---|
| `00-discovery-seed.json` | the discovery-dataset seed response (counts) |
| `01-passport-created.json` | demo passport issued via the **live issuer** (`POST /passports`): the Momiji E8, `urn:unidpp:passport:pilot-e8-j000842`, config = EU + JP profiles |
| `02-passport-view.json` | the passport document after 5 typed events (issuance, 3 corrections, BMS milestone) — also the projector's store copy (`passports/pilot-e8.json`) |
| `03-tier-a-pack.json` | the server-minted Tier-A pack (363 bytes, QR v15-M) + signature block |
| `04-issuer-keyring.json` | the issuer's public anchors (`GET /keyring`) a verifier pins |
| `05-cli-verify.json` | the `unidpp` CLI's offline verification of the pack against the keyring anchor — verdict **pass**, three readings, full coverage |
| `06-archive-snapshot.json` | notarized Tier-C snapshot of the demo passport, **anchored** into the transparency log (receipt, root, tree size inside) |
| `07-projector-view-eu.json` | the EU-lens view: coverage 3/3, reparability 8.1 → class **A** |
| `07-projector-view-jp.json` | the JP-lens view, same passport, same instant: 0.72 kWh → **2.592 MJ**, 8.1 → class **2**, `de.jp.top-runner-class` reported absent (the auditable divergence) |
| `08-gateway-untp.json` | the UNTP VC triad render (source `issuer`, 5 event signatures verified) |
| `09-gateway-en18222.json` | the EN 18222 REST render (full representation) of the GTIN-keyed fixture |
| `10-applicability-2027-06-01.json` | as-of proof: JP only |
| `11-applicability-2028-06-01.json` | as-of proof: EU + JP |
| `pack.hex` | the raw Tier-A carrier bytes |

## The five canonical curls

```sh
# 1. Discovery — which services does the registry know (C3)?
curl -s http://127.0.0.1:8390/services | jq '.services | length'   # -> 8

# 2. As-of applicability — which profiles bind product type momiji:e8 at T?
curl -s 'http://127.0.0.1:8390/applicability?product_type=momiji:e8&at=2027-06-01T00:00:00Z' | jq '[.applicability[].binding.profile_item]'   # -> [JP]
curl -s 'http://127.0.0.1:8390/applicability?product_type=momiji:e8&at=2028-06-01T00:00:00Z' | jq '[.applicability[].binding.profile_item]'   # -> [EU, JP]

# 3. Two-lens view — the same passport under the EU and the JP lens.
curl -s -G 'http://127.0.0.1:8394/view' \
  --data-urlencode 'passport=urn:unidpp:passport:pilot-e8-j000842' \
  --data-urlencode 'profile=urn:unidpp:profile:pilot-jp-lens' \
  --data-urlencode 'actor=importer' --data-urlencode 'at=2028-06-01T00:00:00Z' | jq '.coverage'

# 4. Gateway EN 18222 render — freeDPP-shaped requests, our core.
curl -s 'http://127.0.0.1:8395/en18222/v1/dppsByProductId/4006381333931?representation=full' | jq '.dppStatus'

# 5. Trust keyring — the anchors a verifier pins (the trust service
#    co-signs every GET body in the tree-head domain).
curl -s http://127.0.0.1:8391/keyring | jq '{mode, roles: (.roles | keys)}'
```

## Public URL

Cloudflare named tunnel f45249fa-0190-4ae8-b98c-1dc39c5010c8 (ingress
`pilot.unidpp.org` → `127.0.0.1:8390`; the registry only — the rest
of the stack is loopback by design). `unidpp-stack up` adopts or starts the
tunnel from `tunnel.token`.

**BLOCKED**: DNS record creation needs Zone DNS Edit on the token. Fix (one of):
1. Add "Zone → DNS → Edit" to the unidpp-admin token, then:
   curl -X POST "https://api.cloudflare.com/client/v4/zones/5912805a2be5db3c5070e4063746de9e/dns_records" \
     -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
     -d '{"type":"CNAME","name":"pilot","content":"f45249fa-0190-4ae8-b98c-1dc39c5010c8.cfargotunnel.com","proxied":true}'
2. Or create the CNAME in the dashboard: pilot.unidpp.org → f45249fa-0190-4ae8-b98c-1dc39c5010c8.cfargotunnel.com (proxied)

## Always-on: status, repair, and the PID discipline

The pilot is the always-on demo host. Two commands are the whole
operating loop (TODO.impl 115):

- `./ops/target/release/unidpp-stack status` — the truth: every service (incl. the console
  and the JP node), journal state, and **the public hostname probed
  through the tunnel** — a live tunnel process with a dead origin
  answers 502 and status says so. Exits non-zero when anything is
  down or publicly unreachable.
- `./ops/target/release/unidpp-stack up` — the idempotent repair: healthy services are
  reused untouched, dead ones are restarted (journals replay), and
  every tunnel is re-ensured. A second run is a no-op.

**PID discipline (learned the hard way):** never `pkill`/`killall`
by process name on this box — the pilot's services share binary
names with test instances (a name-based `pkill unidpp-registry`
once killed the pilot's registry and JP node while their tunnels
kept answering 502 publicly). Kill by the exact PID from
`run/*.pid`, or `unidpp-stack down`. After any local test run that
spawned services, `unidpp-stack status` to confirm the pilot's
integrity.

A cron watch is the always-on pattern on a laptop-class host:

```cron
*/10 * * * * cd <pilot-dir> && ./stack.sh start >/dev/null 2>&1
```

## Public trust and log services (TODO.impl 114)

The verifier-facing services are public alongside the registries:
`trust.unidpp.org` → 127.0.0.1:8391 (anchors: `GET /keyring`),
`log.unidpp.org` → 127.0.0.1:8392 (`GET /tree/head`). `stack.sh`
runs both tunnels from `trust-tunnel.token` / `log-tunnel.token` the
moment those files exist (same pattern as the JP and console
tunnels; `stack.sh status` reports them either way).

One-time provisioning per tunnel. Two paths:

**Path A — one authorization click (the cert-login route).** Run
`cloudflared tunnel login` and click the `unidpp.org` zone in the
opened browser tab; `~/.cloudflared/cert.pem` then lets the CLI do
the rest:

```sh
cloudflared tunnel login                      # click the unidpp.org zone
cd ~/src/unidpp/unidpp-pilot-data
cloudflared tunnel create trust-tunnel
cloudflared tunnel route dns trust-tunnel trust.unidpp.org
cloudflared tunnel token trust-tunnel > trust-tunnel.token   # gitignored
cloudflared tunnel create log-tunnel
cloudflared tunnel route dns log-tunnel log.unidpp.org
cloudflared tunnel token log-tunnel > log-tunnel.token
chmod 600 trust-tunnel.token log-tunnel.token
./ops/target/release/unidpp-stack up          # lights both hostnames
```

**Path B — the API** (needs a Cloudflare API token with
Account → Cloudflare Tunnel → Edit and Zone → DNS → Edit — the
token in `~/.config/cloudflare-tokens/unidpp-admin` was verified
**invalid** on 2026-09-22; re-create it first):

```sh
TOKEN=<valid-token>
# 1. create the tunnel, keep the returned id + connector token
curl -X POST "https://api.cloudflare.com/client/v4/accounts/af1920686175ca6d92a677e02bdda75d/cfd_tunnel" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"unidpp-pilot-trust","config_src":"cloudflare"}'
echo '<connector-token>' > trust-tunnel.token    # gitignored
# 2. the DNS record (repeat with "log"/log-tunnel for the log service)
curl -X POST "https://api.cloudflare.com/client/v4/zones/5912805a2be5db3c5070e4063746de9e/dns_records" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"type":"CNAME","name":"trust","content":"<tunnel-id>.cfargotunnel.com","proxied":true}'
# 3. activate: ./stack.sh start && ./stack.sh status
```

## State (2026-09-07)

- Full stack up (8390-8396) via `./stack.sh start`; all services
  healthy; tunnel running (4 connections: fra06, txl01×2, fra18).
- Registry journal: 10 pilot items + 2 bindings + 2 lens profiles +
  the discovery dataset (replays on start).
- Demo artifacts regenerated by `./seed-pilot.sh` (idempotent;
  re-runs reuse the journaled passport).

## The JP national peer node

`registry-jp.unidpp.org` is a second registry instance — the same
binary, its own journal (`jp-registry-journal.jsonl`), its own register
— holding the JP jurisdiction's own data (the road-traffic profile and
its binding, seeded by `./seed-jp.sh`). National registries are peers,
not children of the global registry: the global node keeps what is
tiny and governance-shaped (scheme namespaces, cross-register
mappings); jurisdiction-shaped data lives with the jurisdiction. The
tunnel (`jp-tunnel.token`) and DNS record (`registry-jp.unidpp.org`)
are provisioned once via the Cloudflare API; `./stack.sh start` runs
node and tunnel together with the rest of the stack.

## The admin console and the tenants

`console.unidpp.org` (loopback 8389) is the operator console: the
dashboard, the operator manifest as an editable validated
configuration, the registry browser, passports with inline pack
verification, and the branding preview. The deployment it manages is
declared in `unidpp-operator.yaml` — the same file `unidpp-config`
validates and renders service environments from.

`tenants/` holds whitelabel and sovereign deployments, run by
`tenants/up.sh <name>`:

- **acme** (whitelabel, EU residency): ACME Mobility branding,
  ecdsa-p256 packs, ports 9390/9393/9389.
- **acme-cn** (sovereign, CN residency): ACME 华动 branding, **sm2-only
  packs**, egress none — validated by the manifest itself (sovereign
  profiles refuse external calls without a recorded reason).

Zero code differs between the reference deployment and a tenant: the
manifest is the product.
## The orchestrator

`ops/unidpp-stack` (Rust) is the stack's interface — `up`, `status`,
`down`, `seed-jp` and `tenant <name>` — building, launching,
healthchecking and tearing down the ten services from the operator
manifest (unidpp-config linked as a library, the environment rendered
in-process). The shell scripts remain during the cutover window;
`unidpp-stack` is the primary interface from now on. See `ops/README.md`.

