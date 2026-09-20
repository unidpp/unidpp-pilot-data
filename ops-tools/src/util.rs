//! The shared mechanics of the durability program: timestamps,
//! checksums, the archive interface (tar is a tool, not a shell
//! script — the program shells out to it exactly as the predecessor
//! did), the deployment's own liveness probe, and the small parsers
//! the manifest rewriting needs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

pub fn die(msg: &str) -> ! {
    eprintln!("unidpp-ops: {msg}");
    std::process::exit(1);
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The civil date for a count of days since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`): the program formats UTC stamps
/// itself so the deployment needs no date tool.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A UTC timestamp in the deployment's house format: `%Y%m%dT%H%M%SZ`.
pub fn utc_stamp(epoch: u64) -> String {
    let days = (epoch / 86_400) as i64;
    let rem = epoch % 86_400;
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{y:04}{mo:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// The ISO-8601 minute-precision UTC moment the runbook carries.
pub fn utc_iso(epoch: u64) -> String {
    let days = (epoch / 86_400) as i64;
    let rem = epoch % 86_400;
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// A JSON value as prose, the way Python's `str()` rendered it in
/// the script's messages: a number bare, a string without quotes,
/// an absent value as `None`.
pub fn plain(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    Ok(sha256_hex(&std::fs::read(path)?))
}

// ---------------------------------------------------------------------------
// The archive interface: tar is invoked as a tool, never as a shell
// script — every argument is a separate argv element, so no member
// name ever reaches a shell.
// ---------------------------------------------------------------------------

fn run_tar(args: &[&str]) -> std::process::Output {
    Command::new("tar")
        .args(args)
        .output()
        .unwrap_or_else(|e| die(&format!("tar is required but not usable: {e}")))
}

/// The regular-file members of an archive, in stored order.
pub fn tar_members(archive: &Path) -> Vec<String> {
    let out = run_tar(&["-tzf", &archive.to_string_lossy()]);
    if !out.status.success() {
        die(&format!(
            "cannot list {}: {}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|n| !n.is_empty() && !n.ends_with('/'))
        .map(str::to_string)
        .collect()
}

/// One member's bytes, streamed out of the archive without unpacking
/// anything to disk.
pub fn tar_member(archive: &Path, member: &str) -> Vec<u8> {
    let out = run_tar(&["-xOzf", &archive.to_string_lossy(), member]);
    if !out.status.success() {
        die(&format!(
            "cannot read member {member} from {}: {}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    out.stdout
}

/// A versioned tar.gz of the given paths, rooted at `base` (the
/// members are stored under their relative names).
pub fn tar_create(archive: &Path, base: &Path, members: &[String]) {
    let archive = archive.to_string_lossy().into_owned();
    let base = base.to_string_lossy().into_owned();
    let mut args: Vec<&str> = vec!["-czf", &archive, "-C", &base];
    args.extend(members.iter().map(String::as_str));
    let out = run_tar(&args);
    if !out.status.success() {
        die(&format!(
            "tar failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
}

// ---------------------------------------------------------------------------
// The deployment's own probes.
// ---------------------------------------------------------------------------

/// The consistency point: the log's signed tree head right now, best
/// effort — a deployment without the log running still backs up.
pub fn tree_head() -> String {
    match crate::http::get(8392, "/tree/head", std::time::Duration::from_secs(3)) {
        Some(resp) if resp.status == 200 => {
            match serde_json::from_str::<serde_json::Value>(&resp.body) {
                Ok(doc) => {
                    let size = doc
                        .get("tree_size")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let root = doc.get("root").cloned().unwrap_or(serde_json::Value::Null);
                    format!("size {} root {}", plain(&size), plain(&root))
                }
                Err(_) => "unavailable".to_string(),
            }
        }
        _ => "unavailable".to_string(),
    }
}

pub fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A file is executable when any execute bit is set (the check the
/// script's `[ -x ]` performs).
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

// ---------------------------------------------------------------------------
// Manifest readings: the deployment name and the first-line `name:`
// rewrite need no YAML library — the manifest's own shape carries
// them on single lines.
// ---------------------------------------------------------------------------

/// The deployment name from a manifest, or the fallback the script
/// used when no `name:` line is present.
pub fn manifest_name(manifest: &Path) -> String {
    let text = std::fs::read_to_string(manifest)
        .unwrap_or_else(|e| die(&format!("cannot read {}: {e}", manifest.display())));
    for line in text.lines() {
        if let Some(name) = name_line_value(line) {
            return name;
        }
    }
    "deployment".to_string()
}

/// The value of a line that is entirely `name: <token>` (leading
/// whitespace allowed, the token limited to manifest-name
/// characters), or nothing.
pub fn name_line_value(line: &str) -> Option<String> {
    let rest = line.trim_start();
    let value = rest.strip_prefix("name:")?;
    let value = value.trim();
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    {
        return None;
    }
    Some(value.to_string())
}

/// The paths under `root` whose names match the backup's find
/// predicate, sorted so the payload is deterministic (the find the
/// script used listed them in filesystem order, which was never
/// specified).
pub fn walk_matching(root: &Path, predicate: impl Fn(&str) -> bool + Copy) -> Vec<String> {
    fn walk(
        dir: &Path,
        root: &Path,
        predicate: impl Fn(&str) -> bool + Copy,
        out: &mut Vec<String>,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        names.sort();
        for path in names {
            if path.is_dir() {
                walk(&path, root, predicate, out);
            } else if predicate(
                &path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ) {
                let rel = path
                    .strip_prefix(root)
                    .map(|p| p.to_path_buf())
                    .unwrap_or(path);
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, predicate, &mut out);
    out
}

/// A unique scratch directory under the system temp dir (the
/// mktemp -d of the predecessor, in-process).
pub fn scratch_dir(stem: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "{stem}-{}-{}",
        utc_stamp(now_epoch()),
        std::process::id()
    ));
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| die(&format!("cannot create scratch dir {}: {e}", dir.display())));
    dir
}

/// The archives in a directory sorted newest-name-first (the
/// timestamp in the name makes lexical order chronological).
pub fn archives_newest_first(dir: &Path) -> Vec<PathBuf> {
    let mut names: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().ends_with(".tar.gz"))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names.reverse();
    names
}

fn label_archives(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .map(|n| {
                        let n = n.to_string_lossy();
                        n.starts_with(prefix) && n.ends_with(".tar.gz")
                    })
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// The newest archive carrying a label prefix, by modification time
/// (the `ls -1t | head -1` of the script).
pub fn newest_by_mtime(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = label_archives(dir, prefix)
        .into_iter()
        .filter_map(|p| {
            std::fs::metadata(&p)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (t, p))
        })
        .collect();
    candidates.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    candidates.into_iter().next().map(|(_, p)| p)
}

/// The newest archive carrying a label prefix, by name (the
/// `sort -r | head -1` of the script — the timestamp in the label
/// makes the name order the chronological order).
pub fn newest_by_name(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut names = label_archives(dir, prefix);
    names.sort();
    names.reverse();
    names.into_iter().next()
}
