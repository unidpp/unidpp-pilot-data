//! Disaster recovery: the deployment back at its ORIGINAL shape.
//! Restore lands a backup as a tenant (migration, rehearsal);
//! recover is for catastrophe — the root manifest, the root
//! journals, the seed: the same deployment, standing up again. The
//! current live state is renamed aside, never deleted, and every
//! archive member returns to its original path with no rewriting of
//! tenants or manifests.

use std::path::Path;

use crate::backup;
use crate::util;

/// `recover <archive> [--force]`.
pub fn cmd_recover(root: &Path, archive: &Path, force: bool) {
    backup::verify_archive(archive, false);

    // Journals are held open by running services — refuse while any
    // is up (the rename-aside would strand live state).
    let run = root.join("run");
    if let Ok(entries) = std::fs::read_dir(&run) {
        let mut pidfiles: Vec<(String, String)> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().ends_with(".pid"))
                    .unwrap_or(false)
            })
            .filter_map(|p| {
                let pid = std::fs::read_to_string(&p).ok()?;
                let name = p.file_name()?.to_string_lossy().into_owned();
                Some((name, pid.trim().to_string()))
            })
            .filter(|(_, pid)| !pid.is_empty())
            .collect();
        pidfiles.sort();
        for (pidfile, pid) in pidfiles {
            if let Ok(num) = pid.parse::<u32>() {
                if util::alive(num) {
                    util::die(&format!(
                        "a pilot service is still running (pid {pid}, run/{pidfile}) — stop first: ./stack.sh stop"
                    ));
                }
            }
        }
    }

    // An interrupted recovery leaves *.pre-recover.* behind: that is
    // evidence to investigate, not state to overwrite.
    let leftover = std::fs::read_dir(root).ok().and_then(|entries| {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.contains(".pre-recover."))
            .collect();
        names.sort();
        names.into_iter().next()
    });
    if let Some(leftover) = leftover {
        if !force {
            util::die(&format!(
                "a pre-recover rename-aside already exists ({leftover}) — a prior recovery may be interrupted; inspect it, move it, or pass --force"
            ));
        }
    }

    let stamp = util::utc_stamp(util::now_epoch());
    // Rename the current live state aside — never delete.
    for target in [
        "unidpp-operator.yaml",
        "registry-journal.jsonl",
        "jp-registry-journal.jsonl",
        "run",
        "tenants",
        "seed",
    ] {
        let live = root.join(target);
        if live.exists() {
            let aside = root.join(format!("{target}.pre-recover.{stamp}"));
            std::fs::rename(&live, &aside)
                .unwrap_or_else(|e| util::die(&format!("cannot set {target} aside: {e}")));
            println!("recover: set aside {target} -> {target}.pre-recover.{stamp}");
        }
    }

    // Every member to its ORIGINAL path — no tenant rewriting, no
    // manifest rewriting: this deployment recovering itself.
    let members = util::tar_members(archive);
    for member in &members {
        let out = root.join(member);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| util::die(&format!("cannot create {}: {e}", parent.display())));
        }
        let bytes = util::tar_member(archive, member);
        std::fs::write(&out, bytes)
            .unwrap_or_else(|e| util::die(&format!("cannot write {}: {e}", out.display())));
    }
    println!(
        "recover: unpacked {} member(s) at original paths",
        members.len()
    );

    println!("recover: done. Bring it back:");
    println!("  ./stack.sh start && ./stack.sh status");
    println!("  ./seed-pilot.sh        # idempotent (re-runs reuse journaled state)");
    println!("the rename-aside state survives under *.pre-recover.{stamp} — remove nothing until the recovery is proven.");
}
