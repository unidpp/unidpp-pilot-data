//! The schedule and the retention. The cron lines the program
//! installs name the program's own path (the lines the shell script
//! installed named the script's, which is why operators reinstall
//! when they adopt the program — the marker is the same, the command
//! is the program). The watch line drives the orchestrator's
//! idempotent `up`,
//! which remains the idempotent repair. Prune is report-only by
//! default: deletion is an explicit act.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::util;

const SCHEDULE_MARKER: &str = "unidpp-ops: nightly backup";
const WATCH_MARKER: &str = "unidpp-ops: always-on watch";

fn crontab_text() -> String {
    Command::new("crontab")
        .arg("-l")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn crontab_install(text: &str) {
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| util::die(&format!("cannot run crontab: {e}")));
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap_or_else(|e| util::die(&format!("cannot feed the crontab: {e}")));
    let status = child
        .wait()
        .unwrap_or_else(|e| util::die(&format!("crontab did not finish: {e}")));
    if !status.success() {
        util::die("crontab - rejected the new table");
    }
}

/// The nightly backup line: the program's own absolute path, the
/// deployment root, and the marker the installs key on.
fn schedule_line(root: &Path) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "./unidpp-ops".to_string());
    format!(
        "17 3 * * * cd {} && {} backup >> backups/schedule.log 2>&1 # {SCHEDULE_MARKER}",
        root.display(),
        exe
    )
}

fn watch_line(root: &Path) -> String {
    // The watch drives the orchestrator (the successor of
    // `stack.sh start`): idempotent adopt-or-start every ten minutes.
    format!(
        "*/10 * * * * cd {root} && {root}/ops/target/release/unidpp-stack up >/dev/null 2>&1 # {WATCH_MARKER}",
        root = root.display()
    )
}

/// `schedule [--install|--watch --install|--status]`.
pub fn cmd_schedule(root: &Path, args: &[String]) {
    let first = args.first().map(String::as_str).unwrap_or_default();
    let second = args.get(1).map(String::as_str).unwrap_or_default();
    match first {
        "--watch" => {
            if second != "--install" {
                util::die("usage: schedule --watch --install");
            }
            let current = crontab_text();
            // Idempotent install = the marker's line carries THIS
            // program's command: a stale marked line (the shell
            // script's, or an older binary path) is replaced, not
            // trusted.
            let line = watch_line(root);
            let replaced = current
                .lines()
                .any(|l| l.contains(WATCH_MARKER) && l == line);
            if replaced {
                println!("watch: already installed (the current line)");
                return;
            }
            let mut table = String::new();
            for existing in current.lines().filter(|l| !l.contains(WATCH_MARKER)) {
                table.push_str(existing);
                table.push('\n');
            }
            table.push_str(&line);
            table.push('\n');
            crontab_install(&table);
            println!("watch: installed (*/10 — the idempotent repair)");
            println!("  removal: crontab -l | grep -v 'unidpp-ops: always-on watch' | crontab -");
        }
        "--status" => {
            let current = crontab_text();
            println!(
                "backup: {}",
                if current.contains(SCHEDULE_MARKER) {
                    "installed (nightly at 03:17)"
                } else {
                    "not installed — pass --install"
                }
            );
            println!(
                "watch:  {}",
                if current.contains(WATCH_MARKER) {
                    "installed (the */10 idempotent repair)"
                } else {
                    "not installed — pass --watch --install"
                }
            );
        }
        "" => {
            println!("cron line (print-only; pass --install to add it to the crontab):");
            println!("{}", schedule_line(root));
        }
        "--install" => {
            let current = crontab_text();
            let line = schedule_line(root);
            if current
                .lines()
                .any(|l| l.contains(SCHEDULE_MARKER) && l == line)
            {
                println!("schedule: already installed (the current line)");
                return;
            }
            let mut table = String::new();
            for existing in current.lines().filter(|l| !l.contains(SCHEDULE_MARKER)) {
                table.push_str(existing);
                table.push('\n');
            }
            table.push_str(&line);
            table.push('\n');
            crontab_install(&table);
            println!("schedule: installed (17 3 * * * nightly — see 'crontab -l')");
        }
        other => {
            let _ = other;
            util::die("usage: schedule [--install|--status]");
        }
    }
}

/// `prune [--keep N] [--apply]` — retention over the backups
/// directory, listing everything beyond the newest N archives.
pub fn cmd_prune(backups: &Path, args: &[String]) {
    let mut keep = 14usize;
    let mut apply = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--keep" => match rest.next() {
                Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
                    keep = n.parse().unwrap_or(keep);
                }
                _ => util::die("--keep needs a number"),
            },
            other if other.starts_with("--keep=") => {
                let n = &other["--keep=".len()..];
                if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
                    util::die("--keep needs a number");
                }
                keep = n.parse().unwrap_or(keep);
            }
            "--apply" => apply = true,
            _ => util::die("usage: prune [--keep N] [--apply]"),
        }
    }
    let archives = util::archives_newest_first(backups);
    let total = archives.len();
    println!("prune: {total} backup(s), keeping the newest {keep}");
    for (idx, archive) in archives.iter().enumerate() {
        if idx < keep {
            continue;
        }
        let verb = if apply { "deleted" } else { "candidate" };
        if apply {
            let sidecar = crate::backup::sidecar_of(archive);
            let _ = std::fs::remove_file(archive);
            let _ = std::fs::remove_file(&sidecar);
        }
        println!(
            "  {verb}: {} (+ sidecar)",
            archive
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
    }
    if total > keep && !apply {
        println!("  report-only — pass --apply to delete exactly the listed backup(s)");
    }
}
