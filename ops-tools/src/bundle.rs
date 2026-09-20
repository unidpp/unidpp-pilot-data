//! The on-prem whitelabel artifact: everything a sovereign customer
//! needs on their own host, verified like a backup. The manifest is
//! the model — only the services it declares ship, and the
//! data-only variant carries tenant state alone for a deployment
//! that already has its engine. The bundle's own runner and runbook
//! travel inside it verbatim, exactly as the shell script wrote
//! them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::backup;
use crate::util;

/// The bundle's own runner, byte-identical to the heredoc the shell
/// script embedded: the manifest at the bundle root, every binary
/// from ./bin, self-contained by construction.
const RUN_SH: &str = r##"#!/usr/bin/env bash
# run.sh — the bundle's own runner (start|stop|status).
# The manifest is ./unidpp-operator.yaml; every binary comes from
# ./bin; pidfiles and logs live under ./run.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
CONFIG="$HERE/bin/unidpp-config"
MANIFEST="$HERE/unidpp-operator.yaml"
ACTION="${1:-start}"
mkdir -p "$HERE/run"

case "$ACTION" in
  start) ;;
  stop) ;;
  status) ;;
  *) echo "usage: ./run.sh [start|stop|status]" >&2; exit 2 ;;
esac

[ -x "$CONFIG" ] || { echo "run.sh: no unidpp-config in ./bin" >&2; exit 1; }

for name in $("$CONFIG" services "$MANIFEST"); do
  binary="$HERE/bin/unidpp-$name"
  pidfile="$HERE/run/$name.pid"
  if [ "$ACTION" = stop ]; then
    if [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null; then
      echo "stopped $name"
    else
      echo "$name not running"
    fi
    continue
  fi
  if [ "$ACTION" = status ]; then
    if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
      echo "  $name: running (pid $(cat "$pidfile"))"
    else
      echo "  $name: down"
    fi
    continue
  fi
  if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
    echo "  $name: already running"
    continue
  fi
  [ -x "$binary" ] || { echo "run.sh: no binary for $name at $binary" >&2; exit 1; }
  # shellcheck disable=SC2046
  env $( "$CONFIG" render-env "$name" "$MANIFEST" ) \
    ${name:+$([ "$name" = console ] && printf 'UNIDPP_CONSOLE_MANIFEST=%s ' "$MANIFEST")} \
    nohup "$binary" > "$HERE/run/$name.log" 2>&1 &
  echo $! > "$pidfile"
  echo "  $name: started (pid $(cat "$pidfile"))"
done
"##;

/// The runbook, with the tenant name and the production moment
/// substituted; every other brace and `${VAR}` stays literal, as it
/// did in the script's heredoc.
const RUNBOOK: &str = r##"# @@TENANT@@ — on-prem deployment bundle

This bundle IS a deployment: the manifest (unidpp-operator.yaml) is
the data; the binaries are the engine. Produced @@PRODUCED@@Z.

## First boot

    ./bin/unidpp-config validate unidpp-operator.yaml   # always validate first
    cp .env.template .env                               # fill every ${VAR}
    ./run.sh start                                      # render-env + ./bin binaries
    ./run.sh status

Ports, state files, suites, sovereignty rules: read them from the
manifest — the runner and the services take everything from it.

## Durability

    ./unidpp-ops backup    # snapshot (journals + manifest + seed)
    ./unidpp-ops drill     # the restore rehearsal
    ./unidpp-ops verify <archive>

Full manuals: https://docs.unidpp.org (operations, manifest
reference, every service API).

## Verify this bundle

    shasum -a 256 -c <(python3 -c "import json;print('\n'.join(f'{v}  {k}' for k,v in json.load(open('bundle.json'))['sha256'].items()))")
"##;

/// Every `${VAR}` the manifest references, one `VAR=` per line for
/// the first-boot environment template.
fn env_template(manifest_text: &str) -> String {
    let mut vars = BTreeSet::new();
    let bytes = manifest_text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'$' && bytes[i + 1] == b'{' {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len()
                && (bytes[end].is_ascii_uppercase()
                    || bytes[end].is_ascii_digit()
                    || bytes[end] == b'_')
            {
                end += 1;
            }
            if end > start && end < bytes.len() && bytes[end] == b'}' {
                vars.insert(manifest_text[start..end].to_string());
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    vars.into_iter()
        .map(|v| format!("{v}="))
        .collect::<Vec<_>>()
        .join("\n")
}

fn copy(from: &Path, to: &Path) {
    std::fs::copy(from, to).unwrap_or_else(|e| {
        util::die(&format!(
            "cannot copy {} to {}: {e}",
            from.display(),
            to.display()
        ))
    });
}

/// `bundle <tenant> [--data-only] [--out dir]`.
pub fn cmd_bundle(root: &Path, backups: &Path, args: &[String], quiet: bool) {
    let tenant = match args.first() {
        Some(t) if !t.starts_with('-') => t.clone(),
        _ => util::die("usage: bundle <tenant> [--data-only] [--out dir]"),
    };
    let mut out_dir = backups.to_path_buf();
    let mut data_only = false;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--data-only" => data_only = true,
            "--out" => match rest.next() {
                Some(dir) => out_dir = PathBuf::from(dir),
                None => util::die("usage: bundle <tenant> [--data-only] [--out dir]"),
            },
            _ => util::die("usage: bundle <tenant> [--data-only] [--out dir]"),
        }
    }
    let family = root.parent().unwrap_or(Path::new(".."));
    let config_cli = family.join("unidpp-config/target/release/unidpp-config");
    if !util::is_executable(&config_cli) {
        util::die("unidpp-config binary missing (build ../unidpp-config)");
    }
    let manifest = root
        .join("tenants")
        .join(&tenant)
        .join("unidpp-operator.yaml");
    if !manifest.is_file() {
        util::die(&format!("no tenant manifest at {}", manifest.display()));
    }
    let validated = Command::new(&config_cli)
        .arg("validate")
        .arg(&manifest)
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !validated {
        util::die("tenant manifest does not validate — fix it before bundling");
    }

    let stamp = util::utc_stamp(util::now_epoch());
    let label = if data_only {
        format!("{tenant}-data-{stamp}")
    } else {
        format!("{tenant}-bundle-{stamp}")
    };
    let stage = util::scratch_dir("unidpp-ops-bundle");
    let archive = out_dir.join(format!("{label}.tar.gz"));
    std::fs::create_dir_all(&out_dir)
        .unwrap_or_else(|e| util::die(&format!("cannot create {}: {e}", out_dir.display())));
    let stage_tenant = stage.join(&tenant);
    std::fs::create_dir_all(stage_tenant.join("bin"))
        .unwrap_or_else(|e| util::die(&format!("cannot stage: {e}")));

    // The manifest verbatim — secrets in manifests are ${VAR} refs by
    // construction; render-env substitutes at the target host — plus
    // every journal under the tenant.
    copy(&manifest, &stage_tenant.join("unidpp-operator.yaml"));
    for rel in util::walk_matching(&root.join("tenants").join(&tenant), |n| {
        n.ends_with(".jsonl")
    }) {
        let staged = stage_tenant.join(format!("tenants/{tenant}/{rel}"));
        if let Some(parent) = staged.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| util::die(&format!("cannot stage: {e}")));
        }
        copy(&root.join("tenants").join(&tenant).join(&rel), &staged);
    }

    // Binaries for exactly the declared services (the manifest is
    // the model), the config CLI, the tenant runner, and the ops
    // contract. The data-only artifact carries none of these.
    if !data_only {
        let services_out = Command::new(&config_cli)
            .arg("services")
            .arg(&manifest)
            .output()
            .unwrap_or_else(|e| util::die(&format!("cannot enumerate the declared services: {e}")));
        for svc in String::from_utf8_lossy(&services_out.stdout).lines() {
            let svc = svc.trim();
            if svc.is_empty() {
                continue;
            }
            let bin = family
                .join(format!("unidpp-{svc}"))
                .join("target/release")
                .join(format!("unidpp-{svc}"));
            if !util::is_executable(&bin) {
                util::die(&format!(
                    "binary for declared service '{svc}' missing: {}",
                    bin.display()
                ));
            }
            copy(
                &bin,
                &stage_tenant.join("bin").join(format!("unidpp-{svc}")),
            );
        }
        copy(&config_cli, &stage_tenant.join("bin").join("unidpp-config"));
        copy(&root.join("unidpp-ops"), &stage_tenant.join("unidpp-ops"));
        let run_sh = stage_tenant.join("run.sh");
        std::fs::write(&run_sh, RUN_SH).unwrap_or_else(|e| util::die(&format!("run.sh: {e}")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&run_sh, std::fs::Permissions::from_mode(0o755));
        }
    }

    let manifest_text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| util::die(&format!("cannot read the manifest: {e}")));
    std::fs::write(
        stage_tenant.join(".env.template"),
        env_template(&manifest_text),
    )
    .unwrap_or_else(|e| util::die(&format!(".env.template: {e}")));
    let runbook = RUNBOOK.replacen("@@TENANT@@", &tenant, 1).replacen(
        "@@PRODUCED@@",
        &util::utc_iso(util::now_epoch()),
        1,
    );
    std::fs::write(stage_tenant.join("RUNBOOK.md"), runbook)
        .unwrap_or_else(|e| util::die(&format!("RUNBOOK.md: {e}")));

    util::tar_create(&archive, &stage, std::slice::from_ref(&tenant));

    // The sidecar: the bundle's own checksum map, verified exactly
    // like a backup's.
    let mut members = std::collections::BTreeMap::new();
    for name in util::tar_members(&archive) {
        let bytes = util::tar_member(&archive, &name);
        members.insert(name, util::sha256_hex(&bytes));
    }
    let taken_at = label.rsplit('-').next().unwrap_or_default().to_string();
    let doc = serde_json::json!({
        "bundle": label,
        "tenant": tenant,
        "taken_at_utc": taken_at,
        "consistency_point": {"log_tree_head": "n/a (a bundle is configuration + binaries, not a journal snapshot)"},
        "file_count": members.len(),
        "sha256": members,
    });
    std::fs::write(
        backup::sidecar_of(&archive),
        serde_json::to_string_pretty(&doc).unwrap(),
    )
    .unwrap_or_else(|e| util::die(&format!("cannot write the bundle sidecar: {e}")));
    if !quiet {
        println!("bundle: {label}");
        println!(
            "  files: {}  archive: {}",
            doc["file_count"],
            archive.display()
        );
    }
    let _ = std::fs::remove_dir_all(&stage);
}

/// `decommission <tenant>` — retire a tenant the doctrine-shaped
/// way: prove its state is preserved (a final verified data-only
/// bundle), then move it aside. Never delete — the same rule restore
/// and recover follow.
pub fn cmd_decommission(root: &Path, backups: &Path, tenant: &str) {
    if tenant.contains(".retired.") {
        util::die(&format!(
            "'{tenant}' is already a retired (rename-aside) name"
        ));
    }
    let dir = root.join("tenants").join(tenant);
    if !dir.is_dir() {
        util::die(&format!("no tenant directory at tenants/{tenant}"));
    }
    // Stop its services if running; a stopped tenant is not an error.
    let _ = Command::new(root.join("tenants/up.sh"))
        .args([tenant, "stop"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    cmd_bundle(
        root,
        backups,
        &[tenant.to_string(), "--data-only".to_string()],
        true,
    );
    let bundle = util::newest_by_name(backups, &format!("{tenant}-data-"))
        .unwrap_or_else(|| util::die("decommission: the final bundle was not produced"));
    backup::verify_archive(&bundle, true);

    let stamp = format!("retired.{}", util::utc_stamp(util::now_epoch()));
    let aside = root.join("tenants").join(format!("{tenant}.{stamp}"));
    std::fs::rename(&dir, &aside)
        .unwrap_or_else(|e| util::die(&format!("cannot move the tenant aside: {e}")));
    println!("decommission: tenants/{tenant} -> tenants/{tenant}.{stamp}");
    println!("  preserved: {} (verified)", bundle.display());
    println!(
        "  reversal:  mv tenants/{tenant}.{stamp} tenants/{tenant} && ./tenants/up.sh {tenant} start"
    );
}
