//! The deployment's durability contract, as a program (TODO 246).
//! A deployment is data: the operator manifest, every journal and
//! the seed assets are the deployment, and this program keeps them —
//! backup, verify, restore, the nightly schedule, the always-on
//! watch, retention, the on-prem bundle, the restore rehearsal, the
//! upgrade rehearsal and disaster recovery. It is the successor of
//! the `unidpp-ops` shell script, command for command and format
//! for format: an archive the script made verifies here, and an
//! archive made here verifies under the script's contract. The
//! program's `schedule --install` installs cron lines naming the
//! program's own path, so an operator adopting the program after
//! the script reinstalls the schedule (the marker is unchanged; the
//! command moves to the binary).
//!
//! tar is invoked as a tool, never as a shell script — every
//! argument is a separate argv element — and the checksums are
//! computed in-process through the sha2 crate, which is why the
//! program no longer requires the shasum utility on the operator
//! host. The script itself remains in place until its retirement is
//! proven (TODO 247).

mod backup;
mod bundle;
mod drill;
mod http;
mod recover;
mod rehearse;
mod restore;
mod schedule;
mod util;

use std::path::PathBuf;

const USAGE: &str = r#"unidpp-ops — deployment backup and restore (the durability contract).

  ./unidpp-ops backup  [name]        # snapshot the deployment (default:
                                     #  the tenant named by the root manifest)
  ./unidpp-ops restore <archive> <name> [--force]
  ./unidpp-ops verify <archive>      # checksums only
  ./unidpp-ops schedule [--install]  # the nightly backup cron line
                                     #  (--install merges it idempotently)
  ./unidpp-ops schedule --watch --install
                                     # the always-on watch: */10 the
                                     #  idempotent repair (stack start)
  ./unidpp-ops prune [--keep N] [--apply]
                                     # retention: LIST backups beyond the
                                     #  newest N (report-only unless --apply)
  ./unidpp-ops bundle <tenant> [--data-only] [--out dir]
                                     # the on-prem whitelabel artifact:
                                     #  manifest + declared-service
                                     #  binaries + runner + runbook,
                                     #  verified exactly like a backup
                                     #  (--data-only: the tenant-state
                                     #  artifact for a deployment that
                                     #  already has its engine)
  ./unidpp-ops drill [--keep]        # the restore rehearsal: backup ->
                                     #  restore into a drill tenant ->
                                     #  checksums + byte parity + manifest
                                     #  validation -> JSON drill report

What a backup is: the deployment AS DATA — the operator manifest,
every journal (registry root journal, run/*-journal, every tenant's
journals), and the seed assets — in a versioned tar.gz next to a
JSON sidecar carrying per-file sha256s, the UTC moment, and the log
tree head as the consistency point (journals are append-only; the
tree head at snapshot time is what a restore replays up to).
"#;

fn usage() -> ! {
    print!("{USAGE}");
    std::process::exit(2);
}

fn main() {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    // The binary lives at <pilot>/ops-tools/target/(debug|release)/ —
    // the pilot directory is three levels up from the binary, found
    // the family way: the first ancestor carrying the manifest.
    let root = exe_dir
        .ancestors()
        .nth(3)
        .map(PathBuf::from)
        .filter(|p| p.join("unidpp-operator.yaml").is_file())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .filter(|p| p.join("unidpp-operator.yaml").is_file())
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let backups = root.join("backups");

    // tar is the one external tool the contract leans on; the script
    // checked for it up front and so does the program.
    if !std::process::Command::new("tar")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        util::die("tar is required but not installed");
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = args.iter().map(String::as_str);
    match cmd.next().unwrap_or_default() {
        "backup" => {
            let tenant = cmd.next().filter(|t| !t.is_empty());
            backup::perform_backup(&root, &backups, tenant, false);
        }
        "restore" => {
            let (Some(archive), Some(tenant)) = (cmd.next(), cmd.next()) else {
                util::die("usage: restore <archive> <name> [--force]");
            };
            let force = match cmd.next() {
                None | Some("") => false,
                Some("--force") => true,
                Some(_) => util::die("usage: restore <archive> <name> [--force]"),
            };
            restore::cmd_restore(&root, &PathBuf::from(archive), tenant, force, false);
        }
        "verify" => {
            let Some(archive) = cmd.next() else {
                util::die("usage: verify <archive>");
            };
            backup::verify_archive(&PathBuf::from(archive), false);
        }
        "schedule" => schedule::cmd_schedule(&root, &args[1..]),
        "prune" => schedule::cmd_prune(&backups, &args[1..]),
        "drill" => drill::cmd_drill(&root, &backups, args.get(1).map(String::as_str)),
        "bundle" => {
            if args.len() < 2 {
                util::die("usage: bundle <tenant> [--data-only] [--out dir]");
            }
            bundle::cmd_bundle(&root, &backups, &args[1..], false);
        }
        "rehearse-upgrade" => rehearse::cmd_rehearse(&root, &backups),
        "recover" => {
            let Some(archive) = cmd.next() else {
                util::die("usage: recover <archive> [--force]");
            };
            let force = match cmd.next() {
                None | Some("") => false,
                Some("--force") => true,
                Some(_) => util::die("usage: recover <archive> [--force]"),
            };
            recover::cmd_recover(&root, &PathBuf::from(archive), force);
        }
        "decommission" => {
            let Some(tenant) = cmd.next() else {
                util::die("usage: decommission <tenant>");
            };
            bundle::cmd_decommission(&root, &backups, tenant);
        }
        _ => usage(),
    }
}
