//! The bundle's own runner, as a program: `unidpp-run [start|stop|
//! status]` in a bundle root starts THE ONE process (REARCH R1: the
//! whole estate serves from one binary, every module behind its
//! prefix on one bind). The manifest at the bundle root is the data;
//! the server binary comes from `./bin`; pidfiles and logs live
//! under `./run`; the environment is the merged render of every
//! declared service through `unidpp-config render-env`, plus the
//! deployment's bind and the console's manifest path. The runner is
//! staged into every bundle by `unidpp-ops bundle`.

#![allow(clippy::zombie_processes)] // the server is a daemon by design

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
    // Enumerating the services validates the manifest and names the
    // deployment's modules; their rendered environments merge into
    // the one process's environment.
    let services = Command::new(&config)
        .arg("services")
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| die(&format!("cannot enumerate services: {e}")));
    if !services.status.success() {
        die("unidpp-config services rejected the manifest");
    }
    let bind = Command::new(&config)
        .arg("bind")
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| die(&format!("cannot resolve the deployment bind: {e}")));
    if !bind.status.success() {
        die("unidpp-config bind rejected the manifest");
    }
    let bind = String::from_utf8_lossy(&bind.stdout).trim().to_string();
    act(
        &here,
        &config,
        &manifest,
        &run_dir,
        &action,
        &bind,
        &services.stdout,
    );
}

fn act(
    here: &std::path::Path,
    config: &std::path::Path,
    manifest: &std::path::Path,
    run_dir: &std::path::Path,
    action: &str,
    bind: &str,
    services: &[u8],
) {
    let pidfile = run_dir.join("server.pid");
    let binary = here.join("bin").join("unidpp-server");
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
                println!("stopped server (the deployment is down)");
            } else {
                println!("server not running");
            }
        }
        "status" => {
            if running(&pidfile) {
                let pid = std::fs::read_to_string(&pidfile).unwrap_or_default();
                println!("  server: running (pid {}) on {bind}", pid.trim());
                let modules = String::from_utf8_lossy(services);
                for name in modules.lines() {
                    let name = name.trim();
                    if !name.is_empty() {
                        println!("    {name}: served at /{name}");
                    }
                }
            } else {
                println!("  server: down");
            }
        }
        "start" => {
            if running(&pidfile) {
                println!("  server: already running");
                return;
            }
            if !binary.is_file() {
                die(&format!(
                    "unidpp-run: no unidpp-server at {} (the bundle stages one binary)",
                    binary.display()
                ));
            }
            let modules: Vec<String> = String::from_utf8_lossy(services)
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            let mut command = Command::new(&binary);
            for name in &modules {
                let rendered = Command::new(config)
                    .arg("render-env")
                    .arg(name)
                    .arg(manifest)
                    .output()
                    .unwrap_or_else(|e| die(&format!("render-env for {name}: {e}")));
                if !rendered.status.success() {
                    die(&format!("render-env for {name} rejected the manifest"));
                }
                for line in String::from_utf8_lossy(&rendered.stdout).lines() {
                    if let Some((key, value)) = line.split_once('=') {
                        command.env(key, value);
                    }
                }
            }
            command.env("UNIDPP_SERVER_BIND", bind);
            command.env("UNIDPP_CONSOLE_MANIFEST", manifest);
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(run_dir.join("server.log"))
                .unwrap_or_else(|e| die(&format!("log for the server: {e}")));
            command
                .stdout(
                    log.try_clone()
                        .unwrap_or_else(|e| die(&format!("log handle: {e}"))),
                )
                .stderr(log)
                .process_group(0);
            let child = command
                .spawn()
                .unwrap_or_else(|e| die(&format!("cannot start the server: {e}")));
            let pid = child.id().to_string();
            std::fs::write(&pidfile, &pid)
                .unwrap_or_else(|e| die(&format!("pid file for the server: {e}")));
            println!("  server: started (pid {pid}) on {bind}");
            for name in &modules {
                println!("    {name}: served at /{name}");
            }
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
