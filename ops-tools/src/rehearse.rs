//! The operator's upgrade drill: backup, rebuild the binaries,
//! restart (journals replay), then prove the state equal and the
//! journals append-only. An upgrade never rehearsed is an outage
//! waiting for a calendar slot. The command restarts the stack by
//! design — it is the one operation here that touches the running
//! services, which is why it runs only when an operator asks for it.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::backup::perform_backup;
use crate::util;

fn config_cli(root: &Path) -> std::path::PathBuf {
    root.parent()
        .unwrap_or(Path::new(".."))
        .join("unidpp-config/target/release/unidpp-config")
}

/// A line of the rendered environment shaped `UNIDPP_[A-Z_]*BIND=`
/// (the `^UNIDPP_[A-Z_]*BIND=` match of the script's grep).
fn is_bind_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("UNIDPP_") else {
        return false;
    };
    match rest.find("BIND=") {
        Some(idx) => rest[..idx]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_'),
        None => false,
    }
}

/// The bind port of a declared service, from the manifest's rendered
/// environment — the pilot's ops never hardcode ports.
fn svc_bind(root: &Path, config: &Path, service: &str) -> String {
    let out = Command::new(config)
        .args(["render-env", service])
        .arg(root.join("unidpp-operator.yaml"))
        .output();
    let rendered = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return String::new(),
    };
    let line = rendered.lines().find(|l| is_bind_line(l)).unwrap_or("");
    let value = line.split_once('=').map(|(_, v)| v).unwrap_or("");
    value.split(':').nth(1).unwrap_or("").to_string()
}

/// A JSON GET against a loopback port, or nothing.
fn get_json(port: &str, path: &str) -> Option<serde_json::Value> {
    let port: u16 = port.parse().ok()?;
    let resp = crate::http::get(port, path, std::time::Duration::from_secs(3))?;
    if resp.status != 200 {
        return None;
    }
    serde_json::from_str(&resp.body).ok()
}

fn count_lines(path: &Path) -> Option<usize> {
    std::fs::read_to_string(path)
        .ok()
        .map(|t| t.lines().count())
}

/// The domain readings — what the state IS, not just file sizes —
/// plus the journal line counts. The shape is the rehearsal's
/// before/after contract.
fn readings(root: &Path, binds: &str) -> serde_json::Value {
    let ports: BTreeMap<String, String> = binds
        .split(',')
        .filter_map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();
    let identity = |port: &str| -> serde_json::Value {
        match get_json(port, "/") {
            Some(doc) => {
                let version = doc.get("version").and_then(|v| v.as_str()).unwrap_or("?");
                let build = doc.get("build_id").and_then(|v| v.as_str()).unwrap_or("?");
                serde_json::json!(format!("{version}+{build}"))
            }
            None => serde_json::Value::Null,
        }
    };
    let build_identity: serde_json::Map<String, serde_json::Value> = ports
        .iter()
        .map(|(name, port)| (name.clone(), identity(port)))
        .collect();
    let mut readings = serde_json::Map::new();
    readings.insert(
        "build_identity".into(),
        serde_json::Value::Object(build_identity),
    );
    if let Some(port) = ports.get("registry") {
        let value = get_json(port, "/items?limit=1")
            .and_then(|d| d.get("count").cloned())
            .unwrap_or(serde_json::Value::Null);
        readings.insert("registry_items".into(), value);
    }
    if let Some(port) = ports.get("trust") {
        let value = get_json(port, "/revocations")
            .and_then(|d| {
                d.get("revocations")
                    .and_then(|v| v.as_array())
                    .map(|a| serde_json::json!(a.len()))
            })
            .unwrap_or(serde_json::Value::Null);
        readings.insert("trust_revocations".into(), value);
    }
    if let Some(port) = ports.get("log") {
        let value = get_json(port, "/tree/head")
            .and_then(|d| d.get("tree_size").cloned())
            .unwrap_or(serde_json::Value::Null);
        readings.insert("log_tree_size".into(), value);
    }
    if let Some(port) = ports.get("resolver") {
        let value = get_json(port, "/linksets?limit=1")
            .and_then(|d| d.get("count").cloned())
            .unwrap_or(serde_json::Value::Null);
        readings.insert("resolver_linksets".into(), value);
    }
    let mut lines = serde_json::Map::new();
    for rel in ["registry-journal.jsonl", "jp-registry-journal.jsonl"] {
        let path = root.join(rel);
        if path.exists() {
            if let Some(n) = count_lines(&path) {
                lines.insert(rel.to_string(), serde_json::json!(n));
            }
        }
    }
    for sub in ["run", "tenants"] {
        let base = root.join(sub);
        let mut stack = vec![base];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(|e| e.ok().map(|e| e.path())) {
                if entry.is_dir() {
                    stack.push(entry);
                    continue;
                }
                let Some(name) = entry.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                    continue;
                };
                if name.ends_with("-journal.jsonl")
                    || (sub == "tenants" && name.ends_with(".jsonl"))
                {
                    if let Some(n) = count_lines(&entry) {
                        let rel = entry
                            .strip_prefix(root)
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or(name);
                        lines.insert(rel, serde_json::json!(n));
                    }
                }
            }
        }
    }
    readings.insert("journal_lines".into(), serde_json::Value::Object(lines));
    serde_json::Value::Object(readings)
}

fn py_bool(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

/// `rehearse-upgrade`.
pub fn cmd_rehearse(root: &Path, backups: &Path) {
    let cli = config_cli(root);
    if !util::is_executable(&cli) {
        util::die("unidpp-config binary missing (build ../unidpp-config)");
    }
    let label = format!("upgrade-{}", util::utc_stamp(util::now_epoch()));
    let started = util::now_epoch();

    let services_out = Command::new(&cli)
        .arg("services")
        .arg(root.join("unidpp-operator.yaml"))
        .output()
        .unwrap_or_else(|_| util::die("cannot enumerate the declared services"));
    if !services_out.status.success() {
        util::die("cannot enumerate the declared services");
    }
    let declared: Vec<String> = String::from_utf8_lossy(&services_out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let binds = declared
        .iter()
        .map(|svc| format!("{svc}={}", svc_bind(root, &cli, svc)))
        .collect::<Vec<_>>()
        .join(",");

    println!("rehearse: 1/5 backup");
    perform_backup(root, backups, Some("upgrade"), true);
    let archive = util::newest_by_mtime(backups, "upgrade-")
        .unwrap_or_else(|| util::die("rehearse: no backup archive produced"));

    println!("rehearse: 2/5 before-readings");
    let before = readings(root, &binds);
    let before_path = backups.join(format!("{label}.before.json"));
    std::fs::write(&before_path, serde_json::to_string(&before).unwrap())
        .unwrap_or_else(|e| util::die(&format!("cannot write the before-readings: {e}")));

    println!("rehearse: 3/5 rebuild (the binary swap)");
    let family = root.parent().unwrap_or(Path::new(".."));
    for svc in &declared {
        let repo = format!("unidpp-{svc}");
        let ok = Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(family.join(&repo))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            util::die(&format!("rehearse: rebuild failed: {repo}"));
        }
    }

    println!("rehearse: 4/5 restart (journals replay on the new binaries)");
    for action in ["stop", "start"] {
        let ok = Command::new(root.join("stack.sh"))
            .arg(action)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            util::die(&format!("rehearse: stack.sh {action} failed"));
        }
    }

    println!("rehearse: 5/5 after-readings, parity, append-only proof");
    let after = readings(root, &binds);
    let after_path = backups.join(format!("{label}.after.json"));
    std::fs::write(&after_path, serde_json::to_string(&after).unwrap())
        .unwrap_or_else(|e| util::die(&format!("cannot write the after-readings: {e}")));
    let domain_equal = ["registry_items", "trust_revocations", "log_tree_size"]
        .iter()
        .all(|k| match before.get(k) {
            Some(v) if !v.is_null() => after.get(k) == Some(v),
            _ => true,
        });
    // The build identity is recorded (what the swap actually served),
    // not compared — a version change is the POINT of an upgrade.
    let identity_note = before
        .get("build_identity")
        .and_then(|v| v.as_object())
        .map(|m| {
            let mut note = serde_json::Map::new();
            for name in m.keys() {
                note.insert(
                    name.clone(),
                    serde_json::json!({
                        "before": m.get(name).cloned().unwrap_or_default(),
                        "after": after.get("build_identity").and_then(|a| a.get(name)).cloned().unwrap_or_default(),
                    }),
                );
            }
            serde_json::Value::Object(note)
        })
        .unwrap_or_default();
    let journal_equal = before.get("journal_lines") == after.get("journal_lines");
    // Append-only proof: every journal the backup captured is a byte
    // prefix of the live journal (the upgrade rewrote nothing).
    let mut prefix_ok = 0usize;
    let mut prefix_total = 0usize;
    for member in util::tar_members(&archive) {
        if !member.ends_with(".jsonl") {
            continue;
        }
        prefix_total += 1;
        let archived = util::tar_member(&archive, &member);
        let live = std::fs::read(root.join(&member)).unwrap_or_default();
        if live.starts_with(&archived) {
            prefix_ok += 1;
        }
    }
    let ok = domain_equal && journal_equal && prefix_ok == prefix_total;
    let report = serde_json::json!({
        "upgrade": label,
        "domain_readings_equal": domain_equal,
        "journal_lines_equal": journal_equal,
        "append_only": format!("{prefix_ok}/{prefix_total} backup journals are live-prefix-compatible"),
        "before": before,
        "after": after,
        "build_identity": identity_note,
        "result": if ok { "GREEN" } else { "FAILED" },
    });
    let report_path = backups.join(format!("{label}.report.json"));
    std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap())
        .unwrap_or_else(|e| util::die(&format!("cannot write the rehearsal report: {e}")));
    println!("  domain readings equal: {}", py_bool(domain_equal));
    println!("  journal lines equal:   {}", py_bool(journal_equal));
    println!("  append-only:           {prefix_ok}/{prefix_total}");
    if !ok {
        util::die(&format!(
            "rehearse FAILED (report kept: backups/{label}.report.json)"
        ));
    }
    let _ = std::fs::remove_file(&before_path);
    let _ = std::fs::remove_file(&after_path);
    println!(
        "rehearse: GREEN in {}s — report: backups/{label}.report.json",
        util::now_epoch().saturating_sub(started)
    );
}
