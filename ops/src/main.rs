#![allow(clippy::zombie_processes)] // daemons by design: see the module doc
//! The pilot's orchestration, as a program (TODO 239). A deployment
//! is data: the operator manifest declares the services, unidpp-config
//! renders their environment (linked here as a library — no
//! intermediate binary), and this program builds, launches,
//! healthchecks, seeds and tears the stack down. The successor of
//! `stack.sh`, `seed-jp.sh` and `tenants/up.sh`: one model, one
//! program, no shell.
//!
//! The launched services and tunnels are daemons by design: the
//! orchestrator exits after spawning and the children reparent, so
//! the never-waited spawns below are deliberate (not zombies).
//!
//! State lives where it always lived: `run/` holds the pid files and
//! per-service logs, the registry journals stay at the repo root
//! (they are the seed of record), `passports/` is the projector's
//! store. Every command is idempotent: a healthy listener of the
//! right service is adopted, a foreign listener is named and refused.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use unidpp_config::{load, render_env};

struct Pilot {
    dir: PathBuf,
    family: PathBuf,
}

impl Pilot {
    fn run_dir(&self) -> PathBuf {
        self.dir.join("run")
    }
    fn manifest(&self) -> PathBuf {
        self.dir.join("unidpp-operator.yaml")
    }
    fn bin(&self, repo: &str) -> PathBuf {
        self.family.join(repo).join("target/release").join(repo)
    }
}

// ---------------------------------------------------------------------------
// The service model: one row per listener, its environment derived
// from the manifest where the manifest carries it (the deployment-as-
// data doctrine) and from static wiring where the pilot predates the
// manifest field.
// ---------------------------------------------------------------------------

struct ServiceSpec {
    /// The listener's name (its pid/log stem under run/).
    name: &'static str,
    /// The family repo (also the binary name).
    repo: &'static str,
    /// The loopback port.
    port: u16,
    /// The environment: name, value-or-manifest-derived.
    env: Vec<(&'static str, EnvSource)>,
}

enum EnvSource {
    Fixed(String),
    /// The rendered environment of this manifest service, inlined.
    Rendered(&'static str),
    RunJournal(&'static str),
}

fn services(pilot: &Pilot) -> Vec<ServiceSpec> {
    let dir = pilot.dir.clone();
    let run = pilot.run_dir();
    let passports = dir.join("passports");
    let sp = |s: &str| s.to_string();
    vec![
        ServiceSpec {
            name: "registry",
            repo: "unidpp-registry",
            port: 8390,
            env: vec![
                (
                    "UNIDPP_REGISTRY_BIND",
                    EnvSource::Fixed(sp("127.0.0.1:8390")),
                ),
                (
                    "UNIDPP_REGISTRY_STATE_FILE",
                    EnvSource::Fixed(
                        dir.join("registry-journal.jsonl")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                ),
            ],
        },
        ServiceSpec {
            name: "trust",
            repo: "unidpp-trust",
            port: 8391,
            env: vec![
                ("UNIDPP_TRUST_BIND", EnvSource::Fixed(sp("127.0.0.1:8391"))),
                (
                    "UNIDPP_TRUST_STATE_FILE",
                    EnvSource::RunJournal("trust-journal.jsonl"),
                ),
            ],
        },
        ServiceSpec {
            name: "log",
            repo: "unidpp-log",
            port: 8392,
            env: vec![
                ("UNIDPP_LOG_BIND", EnvSource::Fixed(sp("127.0.0.1:8392"))),
                ("UNIDPP_LOG_ID", EnvSource::Fixed(sp("unidpp-pilot-log-1"))),
                (
                    "UNIDPP_LOG_STATE_FILE",
                    EnvSource::RunJournal("log-journal.jsonl"),
                ),
                (
                    "UNIDPP_LOG_EXTERNAL_TSA_URL",
                    EnvSource::Fixed(
                        std::env::var("UNIDPP_LOG_EXTERNAL_TSA_URL")
                            .unwrap_or_else(|_| "http://timestamp.digicert.com".into()),
                    ),
                ),
            ],
        },
        ServiceSpec {
            name: "issuer",
            repo: "unidpp-issuer",
            port: 8393,
            env: vec![
                ("UNIDPP_ISSUER_BIND", EnvSource::Fixed(sp("127.0.0.1:8393"))),
                (
                    "UNIDPP_ISSUER_STATE_FILE",
                    EnvSource::RunJournal("issuer-journal.jsonl"),
                ),
                (
                    "UNIDPP_ISSUER_REGISTRY_URL",
                    EnvSource::Fixed(sp("http://127.0.0.1:8390")),
                ),
            ],
        },
        ServiceSpec {
            name: "projector",
            repo: "unidpp-projector",
            port: 8394,
            env: vec![
                (
                    "UNIDPP_PROJECTOR_BIND",
                    EnvSource::Fixed(sp("127.0.0.1:8394")),
                ),
                (
                    "UNIDPP_REGISTRY_URL",
                    EnvSource::Fixed(sp("http://127.0.0.1:8390")),
                ),
                (
                    "UNIDPP_PROJECTOR_PASSPORTS_DIR",
                    EnvSource::Fixed(passports.to_string_lossy().into_owned()),
                ),
            ],
        },
        ServiceSpec {
            name: "gateway",
            repo: "unidpp-gateway",
            port: 8395,
            env: vec![("UNIDPP_GATEWAY", EnvSource::Rendered("gateway"))],
        },
        ServiceSpec {
            name: "archive",
            repo: "unidpp-archive",
            port: 8396,
            env: vec![
                (
                    "UNIDPP_ARCHIVE_BIND",
                    EnvSource::Fixed(sp("127.0.0.1:8396")),
                ),
                (
                    "UNIDPP_ARCHIVE_STATE_FILE",
                    EnvSource::RunJournal("archive-journal.jsonl"),
                ),
                (
                    "UNIDPP_ARCHIVE_SNAPSHOT_DIR",
                    EnvSource::Fixed(run.join("archive-snapshots").to_string_lossy().into_owned()),
                ),
                (
                    "UNIDPP_LOG_URL",
                    EnvSource::Fixed(sp("http://127.0.0.1:8392")),
                ),
            ],
        },
        ServiceSpec {
            name: "hub",
            repo: "unidpp-hub",
            port: 8397,
            env: vec![("UNIDPP_HUB", EnvSource::Rendered("hub"))],
        },
        ServiceSpec {
            name: "console",
            repo: "unidpp-console",
            port: 8389,
            env: vec![
                (
                    "UNIDPP_CONSOLE_BIND",
                    EnvSource::Fixed(sp("127.0.0.1:8389")),
                ),
                (
                    "UNIDPP_CONSOLE_MANIFEST",
                    EnvSource::Fixed(pilot.manifest().to_string_lossy().into_owned()),
                ),
            ],
        },
        ServiceSpec {
            name: "jp-registry",
            repo: "unidpp-registry",
            port: 8399,
            env: vec![
                (
                    "UNIDPP_REGISTRY_BIND",
                    EnvSource::Fixed(sp("127.0.0.1:8399")),
                ),
                (
                    "UNIDPP_REGISTRY_STATE_FILE",
                    EnvSource::Fixed(
                        dir.join("jp-registry-journal.jsonl")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                ),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// The loopback HTTP client (the family pattern: hand-rolled, http://
// only, short timeouts).
// ---------------------------------------------------------------------------

struct Response {
    status: u16,
    body: String,
}

fn http(
    method: &str,
    port: u16,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Option<Response> {
    let stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    let mut stream = stream;
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
        method,
        path,
        body.map(str::len).unwrap_or(0),
        body.unwrap_or("")
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split_whitespace().nth(1)?.parse().ok()?;
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Some(Response { status, body })
}

fn healthy(port: u16) -> bool {
    http("GET", port, "/healthz", None, Duration::from_secs(2))
        .map(|r| r.status == 200)
        .unwrap_or(false)
}

/// The discovery document's `service` field names the listener — a
/// healthy /healthz alone cannot tell two family services apart; the
/// console identifies at the well-known path instead.
fn service_id(port: u16) -> Option<String> {
    for path in ["/", "/.well-known/unidpp-service"] {
        if let Some(resp) = http("GET", port, path, None, Duration::from_secs(2)) {
            if resp.status != 200 {
                continue;
            }
            if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&resp.body) {
                if let Some(id) = doc.get("service").and_then(|v| v.as_str()) {
                    if !id.is_empty() {
                        return Some(id.to_string());
                    }
                }
            }
        }
    }
    None
}

fn is_ours(port: u16, repo: &str) -> bool {
    healthy(port) && service_id(port).as_deref() == Some(repo)
}

fn port_taken(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn die(msg: &str) -> ! {
    eprintln!("unidpp-stack: {msg}");
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

fn ensure_bin(pilot: &Pilot, repo: &str) {
    let bin = pilot.bin(repo);
    if bin.is_file() && std::env::var("UNIDPP_FORCE_BUILD").as_deref() != Ok("1") {
        return;
    }
    if !Command::new("cargo")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        die(&format!(
            "cargo not found and no prebuilt binary at {}",
            bin.display()
        ));
    }
    println!("==> building {repo} (release)");
    let ok = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(pilot.family.join(repo))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok || !bin.is_file() {
        die(&format!("build finished but {} is missing", bin.display()));
    }
}

fn rendered_env(pilot: &Pilot, service: &str) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(pilot.manifest())
        .unwrap_or_else(|e| die(&format!("the operator manifest is unreadable: {e}")));
    let manifest =
        load(&text).unwrap_or_else(|e| die(&format!("the operator manifest is invalid: {e}")));
    let rendered = render_env(&manifest, service)
        .unwrap_or_else(|e| die(&format!("render-env {service}: {e}")));
    rendered
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

fn service_environment(pilot: &Pilot, spec: &ServiceSpec) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, source) in &spec.env {
        match source {
            EnvSource::Fixed(value) => out.push((name.to_string(), value.clone())),
            EnvSource::RunJournal(file) => out.push((
                name.to_string(),
                pilot.run_dir().join(file).to_string_lossy().into_owned(),
            )),
            EnvSource::Rendered(service) => {
                // The manifest carries this service's policy: its
                // rendered environment joins wholesale (the static
                // BIND the manifest also carries is included).
                for (k, v) in rendered_env(pilot, service) {
                    out.push((k, v));
                }
            }
        }
    }
    out
}

fn start_service(pilot: &Pilot, spec: &ServiceSpec, tries: u32) {
    let url = format!("http://127.0.0.1:{}", spec.port);
    if is_ours(spec.port, spec.repo) {
        println!("==> {} already healthy on {url} (reusing)", spec.name);
        return;
    }
    if port_taken(spec.port) {
        let holder = service_id(spec.port).unwrap_or_else(|| "?".into());
        die(&format!(
            "port {} is held by '{holder}' but '{}' needs it — stop it first",
            spec.port, spec.repo
        ));
    }
    ensure_bin(pilot, spec.repo);
    println!("==> starting {} on {url}", spec.name);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(pilot.run_dir().join(format!("{}.log", spec.name)))
        .unwrap_or_else(|e| die(&format!("log file: {e}")));
    let mut command = Command::new(pilot.bin(spec.repo));
    command
        .env_clear()
        .envs(service_environment(pilot, spec))
        .stdout(
            log.try_clone()
                .unwrap_or_else(|e| die(&format!("log handle: {e}"))),
        )
        .stderr(log)
        .process_group(0);
    let child = command
        .spawn()
        .unwrap_or_else(|e| die(&format!("{} failed to start: {e}", spec.name)));
    std::fs::write(
        pilot.run_dir().join(format!("{}.pid", spec.name)),
        child.id().to_string(),
    )
    .unwrap_or_else(|e| die(&format!("pid file: {e}")));
    for _ in 0..tries {
        if is_ours(spec.port, spec.repo) {
            return;
        }
        if !alive(child.id()) {
            die(&format!(
                "{} exited during startup (see run/{}.log)",
                spec.name, spec.name
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    die(&format!(
        "{} did not become healthy on {url} (see run/{}.log)",
        spec.name, spec.name
    ));
}

/// A named cloudflared tunnel: absent token skips cleanly; a live
/// pidfile is adopted; else cloudflared starts detached.
fn ensure_named_tunnel(pilot: &Pilot, stem: &str, hostname: &str, port: u16) {
    let token_file = pilot.dir.join(format!("{stem}.token"));
    if std::fs::metadata(&token_file)
        .map(|m| m.len() == 0)
        .unwrap_or(true)
    {
        println!("==> no {stem}.token — {hostname} stays loopback-only");
        return;
    }
    let pidfile = pilot.run_dir().join(format!("{stem}.pid"));
    if let Ok(pid) = std::fs::read_to_string(&pidfile) {
        if let Ok(pid) = pid.trim().parse::<u32>() {
            if alive(pid) {
                println!("==> {stem}: already running (pid {pid})");
                return;
            }
        }
    }
    if !Command::new("cloudflared")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        println!("==> {stem}.token present but cloudflared is not installed — skipping");
        return;
    }
    println!("==> starting cloudflared named tunnel ({hostname} -> {port})");
    let token =
        std::fs::read_to_string(&token_file).unwrap_or_else(|e| die(&format!("{stem}.token: {e}")));
    let token = token.trim().to_string();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(pilot.run_dir().join(format!("{stem}.log")))
        .unwrap_or_else(|e| die(&format!("log file: {e}")));
    let child = Command::new("cloudflared")
        .args([
            "tunnel",
            "run",
            "--token",
            &token,
            "--url",
            &format!("http://127.0.0.1:{port}"),
        ])
        .stdout(
            log.try_clone()
                .unwrap_or_else(|e| die(&format!("log handle: {e}"))),
        )
        .stderr(log)
        .process_group(0)
        .spawn()
        .unwrap_or_else(|e| die(&format!("cloudflared failed to start: {e}")));
    std::fs::write(&pidfile, child.id().to_string()).unwrap();
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_up(pilot: &Pilot) {
    std::fs::create_dir_all(pilot.run_dir()).unwrap();
    std::fs::create_dir_all(pilot.dir.join("passports")).unwrap();
    // Registry first: the issuer forwards to it, the projector reads
    // from it. Journals replay on start; nothing is wiped.
    for spec in services(pilot) {
        start_service(pilot, &spec, 120);
    }
    ensure_named_tunnel(pilot, "tunnel", "registry.unidpp.org", 8390);
    ensure_named_tunnel(pilot, "jp-tunnel", "registry-jp.unidpp.org", 8399);
    ensure_named_tunnel(pilot, "console-tunnel", "console.unidpp.org", 8389);
    ensure_named_tunnel(pilot, "trust-tunnel", "trust.unidpp.org", 8391);
    ensure_named_tunnel(pilot, "log-tunnel", "log.unidpp.org", 8392);
    println!();
    println!("==> stack up: 8390 registry · 8391 trust · 8392 log · 8393 issuer");
    println!("               8394 projector · 8395 gateway · 8396 archive");
    println!("               8397 hub (stateless relay) · 8389 console · 8399 JP peer");
    println!("==> next: unidpp-stack seed-jp   (idempotent)");
}

fn cmd_down(pilot: &Pilot) {
    for name in [
        "console",
        "archive",
        "hub",
        "gateway",
        "projector",
        "issuer",
        "log",
        "trust",
        "registry",
        "jp-registry",
        "tunnel",
        "jp-tunnel",
        "console-tunnel",
        "trust-tunnel",
        "log-tunnel",
    ] {
        let pidfile = pilot.run_dir().join(format!("{name}.pid"));
        if let Ok(pid) = std::fs::read_to_string(&pidfile) {
            let pid = pid.trim().to_string();
            let pid_num: u32 = pid.parse().unwrap_or(0);
            if pid_num > 0 && alive(pid_num) {
                let stopped = Command::new("kill")
                    .arg(&pid)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                println!(
                    "==> {} {}",
                    name,
                    if stopped {
                        format!("stopped (pid {pid})")
                    } else {
                        format!("pid {pid} not stopping")
                    }
                );
            } else {
                println!("==> {name} pid {pid} not running (stale pid file)");
            }
            let _ = std::fs::remove_file(&pidfile);
        }
    }
    println!("==> journals preserved (registry-journal.jsonl, run/*-journal.jsonl)");
}

fn probe_version(port: u16) -> String {
    for path in ["/.well-known/unidpp-service", "/"] {
        if let Some(resp) = http("GET", port, path, None, Duration::from_secs(2)) {
            if resp.status == 200 {
                if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&resp.body) {
                    if let Some(version) = doc.get("version").and_then(|v| v.as_str()) {
                        return version.to_string();
                    }
                }
            }
        }
    }
    "?".into()
}

fn json_count(port: u16, path: &str, key: &str) -> String {
    match http("GET", port, path, None, Duration::from_secs(3)) {
        Some(resp) if resp.status == 200 => {
            match serde_json::from_str::<serde_json::Value>(&resp.body) {
                Ok(doc) => doc
                    .get(key)
                    .and_then(|v| v.as_array())
                    .map(|a| a.len().to_string())
                    .unwrap_or_else(|| "?".into()),
                Err(_) => "?".into(),
            }
        }
        _ => "?".into(),
    }
}

fn cmd_status(pilot: &Pilot) {
    let mut failed = false;
    for spec in services(pilot) {
        if is_ours(spec.port, spec.repo) {
            println!(
                "  {:<10} {:<18} http://127.0.0.1:{}  healthy  v{}",
                spec.name,
                format!("({})", spec.repo),
                spec.port,
                probe_version(spec.port)
            );
        } else if port_taken(spec.port) {
            println!(
                "  {:<10} {:<18} http://127.0.0.1:{}  HELD BY {}",
                spec.name,
                format!("({})", spec.repo),
                spec.port,
                service_id(spec.port).unwrap_or_else(|| "?".into())
            );
            failed = true;
        } else {
            println!(
                "  {:<10} {:<18} http://127.0.0.1:{}  DOWN",
                spec.name,
                format!("({})", spec.repo),
                spec.port
            );
            failed = true;
        }
    }
    if healthy(8390) {
        println!(
            "  registry journal: {} items, {} discovery services",
            json_count(8390, "/items", "items"),
            json_count(8390, "/services", "services")
        );
    }
    if healthy(8392) {
        if let Some(resp) = http("GET", 8392, "/tree/head", None, Duration::from_secs(3)) {
            if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&resp.body) {
                let size = doc.get("tree_size").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("  log tree head: size {size}");
            }
        }
    }
    for (stem, host) in [
        ("tunnel", "registry.unidpp.org"),
        ("jp-tunnel", "registry-jp.unidpp.org"),
        ("console-tunnel", "console.unidpp.org"),
        ("trust-tunnel", "trust.unidpp.org"),
        ("log-tunnel", "log.unidpp.org"),
    ] {
        let pidfile = pilot.run_dir().join(format!("{stem}.pid"));
        let running = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|p| p.trim().parse::<u32>().ok())
            .map(alive)
            .unwrap_or(false);
        if running {
            // A tunnel process can be alive while its origin is down
            // — probe the public hostname itself (curl for TLS).
            let code = Command::new("curl")
                .args([
                    "-s",
                    "-m",
                    "5",
                    "-o",
                    "/dev/null",
                    "-w",
                    "%{http_code}",
                    &format!("https://{host}/healthz"),
                ])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| "?".into());
            if code == "200" {
                println!("  {stem}: running -> https://{host} (public 200)");
            } else {
                println!(
                    "  {stem}: running -> https://{host} (public {code} — origin unreachable?)"
                );
                failed = true;
            }
        } else {
            println!("  {stem}: not running ({host} dark)");
        }
    }
    std::process::exit(if failed { 1 } else { 0 });
}

fn cmd_seed_jp(pilot: &Pilot) {
    let url =
        std::env::var("UNIDPP_JP_REGISTRY_URL").unwrap_or_else(|_| "http://127.0.0.1:8399".into());
    let port: u16 = url
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8399);
    if !healthy(port) {
        die("JP node not healthy (run: unidpp-stack up)");
    }
    let seeds = [
        (
            "/items",
            "seed/items/profile-jp-road-traffic.json",
            "jp-road-traffic profile",
        ),
        (
            "/applicability",
            "seed/bindings/profile-jp-road-traffic-on-momiji-e8.json",
            "jp-road-traffic binding",
        ),
    ];
    for (path, file, label) in seeds {
        let body = std::fs::read_to_string(pilot.dir.join(file))
            .unwrap_or_else(|e| die(&format!("{file}: {e}")));
        let resp = http("POST", port, path, Some(&body), Duration::from_secs(20))
            .unwrap_or_else(|| die(&format!("{label}: no answer")));
        match resp.status {
            201 => println!("  [ok]   {label} registered"),
            409 => println!("  [skip] {label} already present"),
            other => die(&format!("{label}: POST {path} returned {other}")),
        }
    }
    println!(
        "JP node seeded: 1 profile + 1 applicability binding (its own journal, its own register)"
    );
}

fn cmd_tenant(pilot: &Pilot, tenant: &str, action: &str) {
    let manifest = pilot
        .dir
        .join("tenants")
        .join(tenant)
        .join("unidpp-operator.yaml");
    if !manifest.is_file() {
        die(&format!("no manifest at {}", manifest.display()));
    }
    let run = pilot.run_dir().join("tenants").join(tenant);
    std::fs::create_dir_all(&run).unwrap();
    let text =
        std::fs::read_to_string(&manifest).unwrap_or_else(|e| die(&format!("manifest: {e}")));
    let parsed = load(&text).unwrap_or_else(|e| die(&format!("manifest: {e}")));
    let svc = |name: &str, repo: &str, extra: &[(&str, String)]| {
        let pidfile = run.join(format!("{name}.pid"));
        let existing = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|p| p.trim().parse::<u32>().ok())
            .filter(|pid| alive(*pid));
        match action {
            "stop" => {
                if let Some(pid) = existing {
                    let _ = Command::new("kill").arg(pid.to_string()).status();
                    println!("stopped {tenant}/{name}");
                }
                let _ = std::fs::remove_file(&pidfile);
            }
            "status" => {
                println!(
                    "  {name}: {}",
                    match existing {
                        Some(pid) => format!("running (pid {pid})"),
                        None => "down".into(),
                    }
                );
            }
            _ => {
                if let Some(pid) = existing {
                    println!("  {name}: already running (pid {pid})");
                    return;
                }
                let mut env: Vec<(String, String)> = Vec::new();
                if parsed.service_names().contains(&name) {
                    if let Ok(rendered) = render_env(&parsed, name) {
                        for line in rendered.lines() {
                            if let Some((k, v)) = line.split_once('=') {
                                env.push((k.to_string(), v.to_string()));
                            }
                        }
                    }
                }
                for (k, v) in extra {
                    env.push((k.to_string(), v.clone()));
                }
                let log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(run.join(format!("{name}.log")))
                    .unwrap();
                let child = Command::new(pilot.bin(repo))
                    .env_clear()
                    .envs(env)
                    .stdout(log.try_clone().unwrap())
                    .stderr(log)
                    .process_group(0)
                    .spawn()
                    .unwrap_or_else(|e| die(&format!("{name} failed to start: {e}")));
                std::fs::write(&pidfile, child.id().to_string()).unwrap();
                println!("  {name}: started (pid {})", child.id());
            }
        }
    };
    println!("tenant {tenant} ({action}):");
    svc("registry", "unidpp-registry", &[]);
    svc("issuer", "unidpp-issuer", &[]);
    svc(
        "console",
        "unidpp-console",
        &[(
            "UNIDPP_CONSOLE_MANIFEST",
            manifest.to_string_lossy().into_owned(),
        )],
    );
}

fn usage() -> ! {
    die("usage: unidpp-stack <up|down|status|seed-jp> | tenant <name> <up|down|status>")
}

fn main() {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    // The binary lives at <pilot>/ops/target/(debug|release)/ — the
    // pilot directory is two levels up from target.
    let pilot_dir = exe_dir
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .filter(|p| p.join("unidpp-operator.yaml").is_file())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let family = std::env::var("UNIDPP_FAMILY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            pilot_dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".."))
        });
    let pilot = Pilot {
        dir: pilot_dir,
        family,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("up") | Some("start") => cmd_up(&pilot),
        Some("down") | Some("stop") => cmd_down(&pilot),
        Some("status") => cmd_status(&pilot),
        Some("seed-jp") => cmd_seed_jp(&pilot),
        Some("tenant") => match (
            args.get(1).map(String::as_str),
            args.get(2).map(String::as_str),
        ) {
            (Some(tenant), Some("stop") | Some("down")) => cmd_tenant(&pilot, tenant, "stop"),
            (Some(tenant), Some("status")) => cmd_tenant(&pilot, tenant, "status"),
            (Some(tenant), Some("up") | Some("start") | None) => cmd_tenant(&pilot, tenant, "up"),
            _ => usage(),
        },
        _ => usage(),
    }
    let _ = SocketAddr::from(([127, 0, 0, 1], 0));
}
