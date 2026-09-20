//! The backup and its verification: the deployment as data — the
//! operator manifest, every journal, the seed assets — in a versioned
//! tar.gz beside a JSON sidecar carrying every member's sha256, the
//! UTC moment, and the log tree head as the consistency point. An
//! archive made here verifies against the same sidecar contract the
//! shell script established, because the formats are identical.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::util;

/// The sidecar path belonging to an archive (a backup is archive plus
/// sidecar; neither is whole without the other).
pub fn sidecar_of(archive: &Path) -> PathBuf {
    let name = archive.to_string_lossy();
    PathBuf::from(format!(
        "{}.json",
        name.strip_suffix(".tar.gz").unwrap_or(&name)
    ))
}

/// The payload list: manifest, root journals, every journal and
/// script under run/ and tenants/, and every seed file. Derived state
/// (run/ logs, demo/, backups/) is excluded by construction — data,
/// not exhaust.
fn payload(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut present = |rel: &str| {
        if root.join(rel).is_file() {
            files.push(rel.to_string());
        }
    };
    present("unidpp-operator.yaml");
    present("registry-journal.jsonl");
    present("jp-registry-journal.jsonl");
    let predicate = |name: &str| {
        name == "unidpp-operator.yaml" || name.ends_with("-journal.jsonl") || name.ends_with(".sh")
    };
    for sub in ["run", "tenants"] {
        for rel in util::walk_matching(&root.join(sub), predicate) {
            files.push(format!("{sub}/{rel}"));
        }
    }
    for rel in util::walk_matching(&root.join("seed"), |_| true) {
        files.push(format!("seed/{rel}"));
    }
    files
}

/// The member checksums of a freshly written archive: every regular
/// member's sha256, read back out of the archive itself so the
/// sidecar describes what was stored, not what was on disk.
fn member_checksums(archive: &Path) -> BTreeMap<String, String> {
    let mut members = BTreeMap::new();
    for name in util::tar_members(archive) {
        let bytes = util::tar_member(archive, &name);
        members.insert(name, util::sha256_hex(&bytes));
    }
    members
}

/// The backup itself. Returns the label and the archive so the
/// rehearsals can chain onto what was just taken.
pub fn perform_backup(
    root: &Path,
    backups: &Path,
    tenant_arg: Option<&str>,
    quiet: bool,
) -> (String, PathBuf) {
    let tenant = match tenant_arg {
        Some(t) => t.to_string(),
        None => util::manifest_name(&root.join("unidpp-operator.yaml")),
    };
    let stamp = util::utc_stamp(util::now_epoch());
    let label = format!("{tenant}-{stamp}");
    let archive = backups.join(format!("{label}.tar.gz"));
    std::fs::create_dir_all(backups)
        .unwrap_or_else(|e| util::die(&format!("cannot create {}: {e}", backups.display())));

    // The consistency point: the log's signed tree head right now.
    let tree_head = util::tree_head();

    let files = payload(root);
    if files.is_empty() {
        util::die("nothing to back up (no manifest?)");
    }
    util::tar_create(&archive, root, &files);

    let members = member_checksums(&archive);
    let taken_at = label
        .split_once('-')
        .map(|(_, s)| s.to_string())
        .unwrap_or_default();
    let doc = serde_json::json!({
        "backup": label,
        "tenant": tenant,
        "taken_at_utc": taken_at,
        "consistency_point": {"log_tree_head": tree_head},
        "file_count": members.len(),
        "sha256": members,
    });
    std::fs::write(
        sidecar_of(&archive),
        serde_json::to_string_pretty(&doc).unwrap(),
    )
    .unwrap_or_else(|e| util::die(&format!("cannot write the sidecar: {e}")));
    if !quiet {
        println!("backup: {label}");
        println!(
            "  files: {}  consistency point: {tree_head}",
            doc["file_count"]
        );
        println!("  archive: {}", archive.display());
    }
    (label, archive)
}

/// The checksums-only verification every restore and every rehearsal
/// begins with: each member listed in the sidecar must be present in
/// the archive and hash to the recorded value.
pub fn verify_archive(archive: &Path, quiet: bool) {
    if !archive.is_file() {
        util::die(&format!("no such archive: {}", archive.display()));
    }
    let sidecar = sidecar_of(archive);
    if !sidecar.is_file() {
        util::die(&format!(
            "no sidecar for {} (a backup is archive+sidecar)",
            archive.display()
        ));
    }
    let text = std::fs::read_to_string(&sidecar)
        .unwrap_or_else(|e| util::die(&format!("cannot read the sidecar: {e}")));
    let doc: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| util::die(&format!("the sidecar is not valid JSON: {e}")));
    let recorded = doc
        .get("sha256")
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| util::die("the sidecar carries no sha256 map"));
    let present: Vec<String> = util::tar_members(archive);
    let mut bad: Vec<String> = Vec::new();
    for (name, want) in recorded {
        let want = want.as_str().unwrap_or_default();
        if !present.contains(name) {
            bad.push(format!("missing member {name}"));
            continue;
        }
        let got = util::sha256_hex(&util::tar_member(archive, name));
        if got != want {
            bad.push(format!("checksum mismatch: {name}"));
        }
    }
    if !bad.is_empty() {
        eprintln!("unidpp-ops: archive FAILED verification:");
        for b in &bad {
            eprintln!("  {b}");
        }
        std::process::exit(1);
    }
    if !quiet {
        let head = &doc["consistency_point"]["log_tree_head"];
        println!(
            "verified: {} member(s), consistency point {}",
            recorded.len(),
            util::plain(head)
        );
    }
}
