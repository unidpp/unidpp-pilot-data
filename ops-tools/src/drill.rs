//! The restore rehearsal: a backup nobody ever restored is a hope,
//! not a capability. The drill creates and owns its drill tenant,
//! never touches production state, proves checksums and byte parity
//! and manifest validity, and removes what it created unless
//! `--keep` retains it. The recover drill proves the other path: the
//! backup's members unpacked at their ORIGINAL relative shape under
//! a scratch root, byte-verified, without touching the live
//! deployment.

use std::path::Path;
use std::process::Command;

use crate::backup::{perform_backup, sidecar_of};
use crate::restore::rewritten_member_name;
use crate::util;

fn read_sidecar(archive: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(sidecar_of(archive))
        .unwrap_or_else(|e| util::die(&format!("cannot read the sidecar: {e}")));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| util::die(&format!("the sidecar is not valid JSON: {e}")))
}

/// `drill [--keep|--recover]`.
pub fn cmd_drill(root: &Path, backups: &Path, arg: Option<&str>) {
    match arg {
        Some("--recover") => {
            drill_recover(root, backups);
        }
        Some("") | None => drill(root, backups, false),
        Some("--keep") => drill(root, backups, true),
        Some(other) => {
            let _ = other;
            util::die("usage: drill [--keep|--recover]");
        }
    }
}

fn remove_tree(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

fn drill(root: &Path, backups: &Path, keep_tenant: bool) {
    let started = util::now_epoch();
    let drill_tenant = format!("drill-{}", util::utc_stamp(util::now_epoch()));
    let target = root.join("tenants").join(&drill_tenant);
    if target.exists() {
        util::die(&format!(
            "drill tenant exists: {drill_tenant} (refusing to touch it)"
        ));
    }
    std::fs::create_dir_all(backups)
        .unwrap_or_else(|e| util::die(&format!("cannot create {}: {e}", backups.display())));

    println!("drill: 1/4 backup");
    perform_backup(root, backups, Some("drill"), true);
    let archive = util::newest_by_mtime(backups, "drill-")
        .unwrap_or_else(|| util::die("drill: the backup produced no archive"));
    let label = archive
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let label = label.strip_suffix(".tar").unwrap_or(&label).to_string();

    println!("drill: 2/4 restore into tenants/{drill_tenant}");
    crate::restore::cmd_restore(root, &archive, &drill_tenant, false, true);

    println!("drill: 3/4 parity + manifest validation");
    let doc = read_sidecar(&archive);
    let origin = doc.get("tenant").and_then(|v| v.as_str());
    let recorded = doc
        .get("sha256")
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| util::die("the sidecar carries no sha256 map"));
    let mut byte_equal = 0usize;
    let mut checked = 0usize;
    for (name, want) in recorded {
        let name = rewritten_member_name(name, origin, &drill_tenant);
        if name == "unidpp-operator.yaml" {
            continue; // rewritten by design (journal paths, tenant name)
        }
        let out = target.join(&name);
        let got = util::sha256_file(&out)
            .unwrap_or_else(|e| util::die(&format!("parity read {} failed: {e}", out.display())));
        checked += 1;
        if got == want.as_str().unwrap_or_default() {
            byte_equal += 1;
        }
    }
    let report = serde_json::json!({
        "drill": doc.get("backup").cloned().unwrap_or_default(),
        "files": recorded.len(),
        "consistency_point": doc["consistency_point"]["log_tree_head"],
        "checksums": "verified (restore-time)",
        "byte_parity": format!("{byte_equal}/{checked} members byte-identical after restore"),
    });
    let report_path = backups.join(format!("{label}.drill.json"));
    std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap())
        .unwrap_or_else(|e| util::die(&format!("cannot write the drill report: {e}")));
    println!("  byte parity: {byte_equal}/{checked} members identical");
    if byte_equal != checked {
        remove_tree(&target);
        util::die("drill failed at byte parity (report kept)");
    }

    let config_bin = root
        .parent()
        .unwrap_or(Path::new(".."))
        .join("unidpp-config/target/release/unidpp-config");
    let manifest = target.join("unidpp-operator.yaml");
    let manifest_ok = if util::is_executable(&config_bin) {
        let ok = Command::new(&config_bin)
            .arg("validate")
            .arg(&manifest)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            remove_tree(&target);
            util::die("drill failed: the restored manifest does not validate");
        }
        "valid"
    } else {
        "skipped (unidpp-config binary not built)"
    };
    let mut report = report;
    report["manifest_validated"] = serde_json::json!(manifest_ok);
    std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap())
        .unwrap_or_else(|e| util::die(&format!("cannot write the drill report: {e}")));

    println!("drill: 4/4 report");
    if keep_tenant {
        println!("  drill tenant kept: tenants/{drill_tenant}");
    } else {
        remove_tree(&target);
        println!("  drill tenant removed (pass --keep to retain it)");
    }
    println!(
        "drill: GREEN in {}s — report: backups/{label}.drill.json",
        util::now_epoch().saturating_sub(started)
    );
}

/// `drill --recover`: the disaster-recovery rehearsal, unpacked at
/// the original shape under a scratch root and byte-verified against
/// the sidecar.
fn drill_recover(root: &Path, backups: &Path) {
    let label = format!("recover-drill-{}", util::utc_stamp(util::now_epoch()));
    perform_backup(root, backups, Some("drill"), true);
    let archive = util::newest_by_name(backups, "drill-")
        .unwrap_or_else(|| util::die("recover drill: no backup produced"));
    let scratch = util::scratch_dir("unidpp-ops-recover-drill");
    println!(
        "recover-drill: unpack {label} at original shape under {}",
        scratch.display()
    );
    let doc = read_sidecar(&archive);
    let recorded = doc
        .get("sha256")
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| util::die("the sidecar carries no sha256 map"));
    let mut byte_equal = 0usize;
    let mut checked = 0usize;
    for member in util::tar_members(&archive) {
        let out = scratch.join(&member);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| util::die(&format!("cannot create {}: {e}", parent.display())));
        }
        let bytes = util::tar_member(&archive, &member);
        std::fs::write(&out, &bytes)
            .unwrap_or_else(|e| util::die(&format!("cannot write {}: {e}", out.display())));
        checked += 1;
        if util::sha256_hex(&bytes) == recorded.get(&member).and_then(|v| v.as_str()).unwrap_or("")
        {
            byte_equal += 1;
        }
    }
    let archive_name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let report = serde_json::json!({
        "drill": "recover",
        "label": archive_name,
        "shape": "original (no tenant rewriting, no manifest rewriting)",
        "byte_parity": format!("{byte_equal}/{checked} members byte-identical at original paths"),
    });
    std::fs::write(
        backups.join(format!("{label}.json")),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap_or_else(|e| util::die(&format!("cannot write the recover report: {e}")));
    println!("  byte parity at original shape: {byte_equal}/{checked}");
    if byte_equal != checked {
        std::process::exit(1);
    }
    remove_tree(&scratch);
    println!("recover-drill: GREEN — report: backups/{label}.json");
}
