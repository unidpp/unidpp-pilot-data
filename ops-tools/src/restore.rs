//! The restore: a verified archive landing as a tenant. Restore
//! refuses to overwrite an existing tenant without `--force`, and
//! before it unpacks a single file it has verified every checksum
//! the sidecar carries. The restored manifest becomes the tenant's
//! own — its journal paths are rewritten into the tenant (a restored
//! deployment must never write the origin's journals) and its name
//! is rebased to the tenant.

use std::path::{Path, PathBuf};

use crate::backup::{self, sidecar_of};
use crate::util;

/// The manifest rewrite, ported from the script's ordered set of
/// `state_file:` substitutions: the root journals and every run/
/// journal move under the tenant, and any tenant-qualified state
/// file is rebased to the restored tenant. The alternatives are
/// tried at each occurrence in the script's order, which leaves the
/// result identical to the sequential substitutions.
fn rewrite_state_files(text: &str, tenant: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    while let Some(idx) = rest.find("state_file:") {
        let (before, after) = rest.split_at(idx);
        out.push_str(before);
        out.push_str("state_file:");
        let value = &after["state_file:".len()..];
        let trimmed = value.trim_start_matches([' ', '\t']);
        if let Some(tail) = trimmed.strip_prefix("registry-journal.jsonl") {
            out.push_str(&format!(" tenants/{tenant}/registry-journal.jsonl"));
            rest = tail;
        } else if let Some(tail) = trimmed.strip_prefix("jp-registry-journal.jsonl") {
            out.push_str(&format!(" tenants/{tenant}/jp-registry-journal.jsonl"));
            rest = tail;
        } else if let Some(after_tenants) = trimmed.strip_prefix("tenants/") {
            let name_len = after_tenants
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || "._-".contains(*c))
                .count();
            let after_name = &after_tenants[name_len..];
            if let Some(tail) = after_name.strip_prefix('/') {
                out.push_str(&format!(" tenants/{tenant}/"));
                rest = tail;
            } else {
                // The token after `tenants/` is not a path segment
                // the rule recognizes — leave the occurrence alone.
                rest = &after["state_file:".len()..];
            }
        } else if let Some(after_run) = trimmed.strip_prefix("run/") {
            // The script's `[a-z-]+` runs greedy and then backtracks
            // against the required `-journal.jsonl`, which lands on
            // the suffix's first occurrence: the service name is
            // whatever precedes it, provided it names a service.
            match after_run.find("-journal.jsonl") {
                Some(n)
                    if n > 0
                        && after_run[..n]
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c == '-') =>
                {
                    let service = &after_run[..n];
                    out.push_str(&format!(" tenants/{tenant}/{service}-journal.jsonl"));
                    rest = &after_run[n + "-journal.jsonl".len()..];
                }
                _ => rest = &after["state_file:".len()..],
            }
        } else {
            rest = &after["state_file:".len()..];
        }
    }
    out.push_str(rest);
    out
}

/// The first `name: <token>` line is rebased to the restored tenant
/// (the `count=1` of the script's substitution).
fn rewrite_name(text: &str, tenant: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for line in lines.iter_mut() {
        if util::name_line_value(line).is_some() {
            let trimmed = line.trim_start();
            let ws_len = line.len() - trimmed.len();
            let after = &trimmed["name:".len()..];
            let inner = &after[..after.len() - after.trim_start_matches([' ', '\t']).len()];
            *line = format!("{}name:{inner}{tenant}", &line[..ws_len]);
            break;
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The member-name mapping restore applies: only the backup's OWN
/// tenant journals rewrite into the restored one (they are the
/// deployment's live state); other tenants' journals are archive
/// material and keep their original tenant path — rewriting them all
/// would collide sibling tenants onto the same filenames.
pub fn rewritten_member_name(name: &str, origin: Option<&str>, tenant: &str) -> String {
    let mut parts: Vec<&str> = name.split('/').collect();
    if parts.len() >= 2 && parts[0] == "tenants" && Some(parts[1]) == origin {
        parts[1] = tenant;
        return parts.join("/");
    }
    name.to_string()
}

/// Unpack the archive's members into the tenant directory. Bundle
/// layout (every member under one shared top-level directory) is
/// stripped first, then the backup rules apply.
fn unpack_into_tenant(archive: &Path, target: &Path, tenant: &str, quiet: bool) {
    let sidecar_text = std::fs::read_to_string(sidecar_of(archive))
        .unwrap_or_else(|e| util::die(&format!("cannot read the sidecar: {e}")));
    let doc: serde_json::Value = serde_json::from_str(&sidecar_text)
        .unwrap_or_else(|e| util::die(&format!("the sidecar is not valid JSON: {e}")));
    let origin = doc
        .get("tenant")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let members = util::tar_members(archive);
    let firsts: Vec<&str> = members
        .iter()
        .map(|m| m.split('/').next().unwrap_or(""))
        .collect();
    let strip = if !members.is_empty()
        && members.iter().all(|m| m.contains('/'))
        && firsts.windows(2).all(|w| w[0] == w[1])
    {
        format!("{}/", firsts[0])
    } else {
        String::new()
    };
    for member in &members {
        let name = rewritten_member_name(
            member.strip_prefix(strip.as_str()).unwrap_or(member),
            origin.as_deref(),
            tenant,
        );
        let out = target.join(&name);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| util::die(&format!("cannot create {}: {e}", parent.display())));
        }
        let bytes = util::tar_member(archive, member);
        std::fs::write(&out, bytes)
            .unwrap_or_else(|e| util::die(&format!("cannot write {}: {e}", out.display())));
    }
    if !quiet {
        println!(
            "restored into tenants/{tenant} (origin tenant: {})",
            origin.as_deref().unwrap_or("None")
        );
    }
}

/// `restore <archive> <name> [--force]`.
pub fn cmd_restore(root: &Path, archive: &Path, tenant: &str, force: bool, quiet: bool) {
    backup::verify_archive(archive, quiet);
    let target = root.join("tenants").join(tenant);
    if target.exists() && !force {
        util::die(&format!(
            "tenant '{tenant}' already exists — pass --force to replace it (the existing directory is renamed aside, never deleted)"
        ));
    }
    if target.exists() {
        let aside = PathBuf::from(format!(
            "{}.pre-restore.{}",
            target.display(),
            util::utc_stamp(util::now_epoch())
        ));
        std::fs::rename(&target, &aside).unwrap_or_else(|e| {
            util::die(&format!("cannot rename {} aside: {e}", target.display()))
        });
    }
    std::fs::create_dir_all(&target)
        .unwrap_or_else(|e| util::die(&format!("cannot create {}: {e}", target.display())));
    unpack_into_tenant(archive, &target, tenant, quiet);

    // The restored manifest becomes the tenant's own.
    let manifest = target.join("unidpp-operator.yaml");
    if manifest.is_file() {
        let text = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|e| util::die(&format!("cannot read the restored manifest: {e}")));
        let rewritten = rewrite_name(&rewrite_state_files(&text, tenant), tenant);
        std::fs::write(&manifest, rewritten)
            .unwrap_or_else(|e| util::die(&format!("cannot write the restored manifest: {e}")));
        if !quiet {
            println!("restored manifest rewritten: journals under tenants/{tenant}, name {tenant}");
        }
    }
    if !quiet {
        println!("review the binds in its manifest, then: ./tenants/up.sh {tenant}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rewrite lands exactly where the script's ordered set of
    /// regular expressions landed: the two root journals move under
    /// the tenant, a run/ journal keeps its service name (the greedy
    /// `[a-z-]+` backtracks against the `-journal.jsonl` suffix), a
    /// tenant-qualified state file rebases, and the first `name:`
    /// line takes the tenant's name.
    #[test]
    fn rewrite_matches_the_script() {
        let manifest = "api_version: unidpp.org/v1\ndeployment:\n  name: unidpp-reference\nservices:\n  registry:\n    bind: 127.0.0.1:8390\n    state_file: registry-journal.jsonl\n  log:\n    state_file: run/log-journal.jsonl\n  archive:\n    state_file: run/archive-journal.jsonl\n  other:\n    state_file: tenants/acme-cn/registry-journal.jsonl\n";
        let tenant = "drill-x";
        let rewritten = rewrite_name(&rewrite_state_files(manifest, tenant), tenant);
        let lines: Vec<&str> = rewritten.lines().collect();
        assert_eq!(lines[2], "  name: drill-x");
        assert_eq!(
            lines[6],
            "    state_file: tenants/drill-x/registry-journal.jsonl"
        );
        assert_eq!(
            lines[8],
            "    state_file: tenants/drill-x/log-journal.jsonl"
        );
        assert_eq!(
            lines[10],
            "    state_file: tenants/drill-x/archive-journal.jsonl"
        );
        assert_eq!(
            lines[12],
            "    state_file: tenants/drill-x/registry-journal.jsonl"
        );
        assert!(!rewritten.contains("acme-cn"));
        assert!(!rewritten.contains("run/"));
    }

    /// A journal name that itself contains the suffix shape still
    /// resolves to its service name, the way the backtracking
    /// expression resolved it.
    #[test]
    fn rewrite_unpicks_the_journal_suffix() {
        let rewritten = rewrite_state_files("    state_file: run/trust-journal.jsonl\n", "t2");
        assert_eq!(
            rewritten,
            "    state_file: tenants/t2/trust-journal.jsonl\n"
        );
    }

    /// A state file the rules do not recognize passes through
    /// untouched rather than being mangled.
    #[test]
    fn rewrite_leaves_unknown_state_files_alone() {
        let line = "    state_file: elsewhere/journal.jsonl\n";
        assert_eq!(rewrite_state_files(line, "t3"), line);
    }
}
