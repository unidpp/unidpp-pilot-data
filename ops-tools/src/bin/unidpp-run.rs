//! The bundle's own runner, as a program: `unidpp-run [start|stop|
//! status]` in a bundle root succeeds the embedded `run.sh` heredoc
//! (retired with the family's script-free pass). The contract is the
//! same: the manifest at the bundle root, every binary from `./bin`,
//! pidfiles and logs under `./run`, the environment rendered through
//! `unidpp-config render-env`, and the console carrying its own
//! manifest path. The runner is staged into every bundle by
//! `unidpp-ops bundle`.

#![allow(clippy::zombie_processes)] // services are daemons by design

use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn main() {
    let action = std::env::args().nth(1).unwrap_or_else(|| "start".into());
    if !matches!(action.as_str(), "start" | "stop" | "status") {
        eprintln!("usage: unidpp-run [start|stop|status]");
        std::process::exit(2);
    }
    let here = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let config = here.join("bin").join("unidpp-config");
    let manifest = here.join("unidpp-operator.yaml");
    let run_dir = here.join("run");
    std::fs::create_dir_all(&run_dir)
        .unwrap_or_else(|e| die(&format!("cannot create {}: {e}", run_dir.display())));
    if !config.is_file() {
        die(&format!(
            "unidpp-run: no unidpp-config in ./bin (looked at {})",
            config.display()
        ));
    }

    let services = Command::new(&config)
        .arg("services")
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| die(&format!("cannot enumerate services: {e}")));
    if !services.status.success() {
        die("unidpp-config services rejected the manifest");
    }
    for name in String::from_utf8_lossy(&services.stdout).lines() {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        act(&here, &config, &manifest, &run_dir, &action, name);
    }
}

fn act(
    here: &std::path::Path,
    config: &std::path::Path,
    manifest: &std::path::Path,
    run_dir: &std::path::Path,
    action: &str,
    name: &str,
) {
    let pidfile = run_dir.join(format!("{name}.pid"));
    let binary = here.join("bin").join(format!("unidpp-{name}"));
    match action {
        "stop" => {
            let pid = std::fs::read_to_string(&pidfile)
                .ok()
                .map(|p| p.trim().to_string());
            let stopped = pid
                .as_ref()
                .and_then(|p| Command::new("kill").arg(p).status().ok())
                .is_some_and(|s| s.success());
            if stopped {
                println!("stopped {name}");
            } else {
                println!("{name} not running");
            }
        }
        "status" => {
            if running(&pidfile) {
                let pid = std::fs::read_to_string(&pidfile).unwrap_or_default();
                println!("  {name}: running (pid {})", pid.trim());
            } else {
                println!("  {name}: down");
            }
        }
        "start" => {
            if running(&pidfile) {
                println!("  {name}: already running");
                return;
            }
            if !binary.is_file() {
                die(&format!(
                    "unidpp-run: no binary for {name} at {}",
                    binary.display()
                ));
            }
            let rendered = Command::new(config)
                .arg("render-env")
                .arg(name)
                .arg(manifest)
                .output()
                .unwrap_or_else(|e| die(&format!("render-env for {name}: {e}")));
            if !rendered.status.success() {
                die(&format!("render-env for {name} rejected the manifest"));
            }
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(run_dir.join(format!("{name}.log")))
                .unwrap_or_else(|e| die(&format!("log for {name}: {e}")));
            let mut command = Command::new(&binary);
            for line in String::from_utf8_lossy(&rendered.stdout).lines() {
                if let Some((key, value)) = line.split_once('=') {
                    command.env(key, value);
                }
            }
            if name == "console" {
                command.env("UNIDPP_CONSOLE_MANIFEST", manifest);
            }
            command
                .stdout(
                    log.try_clone()
                        .unwrap_or_else(|e| die(&format!("log handle: {e}"))),
                )
                .stderr(log)
                .process_group(0);
            let child = command
                .spawn()
                .unwrap_or_else(|e| die(&format!("cannot start {name}: {e}")));
            let pid = child.id().to_string();
            std::fs::write(&pidfile, &pid)
                .unwrap_or_else(|e| die(&format!("pid file for {name}: {e}")));
            println!("  {name}: started (pid {pid})");
        }
        _ => unreachable!(),
    }
}

fn running(pidfile: &std::path::Path) -> bool {
    let pid = std::fs::read_to_string(pidfile).unwrap_or_default();
    let pid = pid.trim();
    if pid.is_empty() {
        return false;
    }
    Command::new("kill")
        .args(["-0", pid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}
