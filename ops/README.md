# unidpp-stack — the pilot's orchestration

A deployment is data: the operator manifest declares the services,
`unidpp-config` renders their environment (linked as a library — the
rendering happens in-process), and this program builds, launches,
healthchecks, seeds and tears the stack down. It is the successor of
`stack.sh`, `seed-jp.sh` and `tenants/up.sh`, run from the pilot
directory (or anywhere — the pilot directory is found from the
binary's location):

```sh
cargo build --release          # in ops/
./ops/target/release/unidpp-stack up          # idempotent: adopt-or-start + wait healthy
./ops/target/release/unidpp-stack status      # per-service health + journal + tunnels (exit 1 on any down)
./ops/target/release/unidpp-stack down        # stop everything, journals preserved
./ops/target/release/unidpp-stack seed-jp     # the JP peer's own data (idempotent)
./ops/target/release/unidpp-stack tenant acme [up|down|status]
```

State lives where it always lived: `run/` holds the pid files and
per-service logs, the registry journals stay at the pilot root (they
are the seed of record), `passports/` is the projector's store. A
healthy listener of the right service is adopted; a foreign listener
is named and refused. The gateway and the hub launch from the
manifest's rendered environment (the deployment-as-data doctrine);
the remaining services' wiring is declared in the service table of
`src/main.rs`, awaiting their manifest fields.

The shell scripts remain in place during the cutover window; the
walkthrough seed (`seed-pilot.sh`) ports next — it is the last piece
before the scripts retire.
