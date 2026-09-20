//! The walkthrough seed (the successor of `seed-pilot.sh`): seed the
//! registry, drive the live issuer through the demonstration
//! passport, verify the pack through the CLI, and write the demo
//! artifacts. Idempotent and re-runnable against a running stack —
//! creations tolerate 409, events append only up to the expected
//! sequence, the binding check looks into the future.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::{die, healthy, Pilot};

const REGISTRY: u16 = 8390;
const ISSUER: u16 = 8393;
const PROJECTOR: u16 = 8394;
const GATEWAY: u16 = 8395;
const ARCHIVE: u16 = 8396;

const DEMO_ID: &str = "local:momiji:e8/J-000842";
const DEMO_TYPE_REF: &str = "momiji:e8";
const DEMO_URN: &str = "urn:unidpp:passport:pilot-e8-j000842";
const DEMO_EO: &str = "urn:unidpp:actor:momiji-mobility";
const EU_LENS: &str = "urn:unidpp:profile:pilot-eu-lens";
const JP_LENS: &str = "urn:unidpp:profile:pilot-jp-lens";
const GTIN_FIXTURE: &str = "4006381333931";
const VIEW_AT: &str = "2028-06-01T00:00:00Z";

use crate::http::{self, HttpText};

fn get(port: u16, path: &str) -> Option<HttpText> {
    http::get(port, path, Duration::from_secs(20))
}

fn post(port: u16, path: &str, body: &str) -> Option<HttpText> {
    http::post(port, path, body, Duration::from_secs(20))
}

fn parse(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

fn s<'a>(doc: &'a serde_json::Value, path: &[&str]) -> &'a serde_json::Value {
    let mut cur = doc;
    for key in path {
        cur = match cur {
            serde_json::Value::Array(items) => items
                .get(key.parse::<usize>().unwrap_or(usize::MAX))
                .unwrap_or(&serde_json::Value::Null),
            other => other.get(key).unwrap_or(&serde_json::Value::Null),
        };
    }
    cur
}

fn text(doc: &serde_json::Value, path: &[&str]) -> String {
    let value = s(doc, path);
    match value {
        serde_json::Value::Null => "?".into(),
        serde_json::Value::String(v) => v.clone(),
        other => other.to_string(),
    }
}

fn count(doc: &serde_json::Value, path: &[&str]) -> usize {
    s(doc, path).as_array().map(|a| a.len()).unwrap_or(0)
}

/// Percent-encode everything outside the unreserved set (the URNs ride
/// URL paths).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn artifact_dir(pilot: &Pilot) -> std::path::PathBuf {
    let dir = pilot.dir.join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_artifact(dir: &Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap_or_else(|e| die(&format!("{name}: {e}")));
}

fn expect_created(label: &str, resp: HttpText, ok: &[u16]) -> HttpText {
    if ok.contains(&resp.status) {
        resp
    } else {
        die(&format!(
            "{label} -> {}: {}",
            resp.status,
            resp.body.chars().take(300).collect::<String>()
        ));
    }
}

fn sorted_profiles(doc: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = doc["applicability"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| text(b, &["binding", "profile_item"]))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

pub fn run(pilot: &Pilot) {
    let dir = artifact_dir(pilot);
    for (name, port) in [
        ("registry", REGISTRY),
        ("issuer", ISSUER),
        ("projector", PROJECTOR),
        ("gateway", GATEWAY),
        ("archive", ARCHIVE),
    ] {
        if !healthy(port) {
            die(&format!("{name} is not healthy — run: unidpp-stack up"));
        }
    }
    let cli = pilot.family.join("unidpp-cli/target/release/unidpp");
    if !cli.is_file() {
        die(&format!(
            "unidpp CLI missing at {} (cd unidpp-cli && cargo build --release)",
            cli.display()
        ));
    }

    // [1] discovery seed (C3/C4/C5 + C1 units)
    println!("== [1] discovery seed (C3/C4/C5 + C1 units)");
    let units = get(REGISTRY, "/units")
        .map(|r| count(&parse(&r.body), &["items"]))
        .unwrap_or(0);
    if units == 0 {
        let resp = expect_created(
            "POST /admin/seed",
            post(REGISTRY, "/admin/seed", "{}").expect("seed answered"),
            &[200, 201],
        );
        write_artifact(&dir, "00-discovery-seed.json", &resp.body);
        let doc = parse(&resp.body);
        let summary: Vec<String> = doc["counts"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| format!("{k}={v}")).collect())
            .unwrap_or_default();
        println!("  seeded: {}", summary.join(", "));
    } else {
        println!("  already present ({units} units — journal replay)");
    }

    // [2] item seed (10 pilot items)
    println!("== [2] item seed (10 pilot items)");
    for entry in sorted_json_files(&pilot.dir.join("seed/items")) {
        let body = std::fs::read_to_string(&entry).unwrap();
        let item = text(&parse(&body), &["item_id"]);
        let resp = post(REGISTRY, "/items", &body).expect("items answered");
        match resp.status {
            201 => println!("  registered {item}"),
            409 => println!("  present    {item}"),
            other => die(&format!(
                "POST /items -> {other}: {}",
                resp.body.chars().take(300).collect::<String>()
            )),
        }
    }

    // [3] lens seed (projector-shaped lens profiles)
    println!("== [3] lens seed (projector-shaped lens profiles)");
    for entry in sorted_json_files(&pilot.dir.join("seed/lens")) {
        let body = std::fs::read_to_string(&entry).unwrap();
        let item = text(&parse(&body), &["item_id"]);
        let resp = post(REGISTRY, "/profiles", &body).expect("profiles answered");
        match resp.status {
            201 => println!("  registered {item}"),
            409 => println!("  present    {item}"),
            other => die(&format!(
                "POST /profiles -> {other}: {}",
                resp.body.chars().take(300).collect::<String>()
            )),
        }
    }

    // [4] applicability bindings (dated, on product type momiji:e8).
    // The check looks into the future — both windows start 2027/2028,
    // and the registry deliberately does not dedupe bindings.
    println!("== [4] applicability bindings (dated, on product type momiji:e8)");
    for entry in sorted_json_files(&pilot.dir.join("seed/bindings")) {
        let body = std::fs::read_to_string(&entry).unwrap();
        let doc = parse(&body);
        let profile = text(&doc, &["profile_id"]);
        let subject = text(&doc, &["product_type"]);
        let bound = get(
            REGISTRY,
            &format!("/applicability?product_type={subject}&at=2030-01-01T00:00:00Z"),
        )
        .map(|r| sorted_profiles(&parse(&r.body)).contains(&profile))
        .unwrap_or(false);
        if bound {
            println!("  present    {profile} on {subject}");
        } else {
            let resp = expect_created(
                &format!("POST /applicability ({profile})"),
                post(REGISTRY, "/applicability", &body).expect("bindings answered"),
                &[201],
            );
            let _ = resp;
            println!(
                "  bound      {profile} on {subject} (from {})",
                text(&doc, &["effective_from"])
            );
        }
    }

    // [5] demo passport via the LIVE issuer.
    println!("== [5] demo passport (live issuer)");
    let create = serde_json::json!({
        "identity": DEMO_ID,
        "type_ref": DEMO_TYPE_REF,
        "capability": "S2",
        "eo_id": DEMO_EO,
        "resolver_uri": "https://resolver.unidpp.org/r/pilot-e8-j000842",
        "passport_id": DEMO_URN,
        "config": ["urn:unidpp:profile:eu-machinery-battery", "urn:unidpp:profile:jp-road-traffic"],
    });
    let enc = encode(DEMO_URN);
    let resp = expect_created(
        "POST /passports",
        post(ISSUER, "/passports", &create.to_string()).expect("issuer answered"),
        &[201, 409],
    );
    if resp.status == 201 {
        write_artifact(&dir, "01-passport-created.json", &resp.body);
        println!("  issued {DEMO_URN}");
    } else {
        println!("  already issued {DEMO_URN} (issuer journal replay)");
    }

    // The events, in order. The milestone is stamped at seed time —
    // the S2 BMS dumps its counter history on physical read (story
    // beat B5), and a fresh stamp keeps the minted pack inside the
    // Tier-A freshness window.
    let now = now_stamp();
    let events: [(&str, serde_json::Value, &str, &str, &str); 5] = [
        (
            "issuance",
            serde_json::json!({"Issuance": {"derived": false, "inputs": []}}),
            "momiji-mobility",
            "issuing authority",
            "2026-07-14T08:00:00Z",
        ),
        (
            "correction",
            serde_json::json!({"Correction": {"field": "subject.markets", "prior_value": "", "new_value": "EU,JP", "reason": "market placement declaration (EU, JP)"}}),
            "momiji-mobility",
            "economic operator",
            "2026-07-14T09:00:00Z",
        ),
        (
            "correction",
            serde_json::json!({"Correction": {"field": "de.dpp.operator-id", "prior_value": "", "new_value": "urn:unidpp:actor:momiji-mobility", "reason": "EU/JP DPP operator identifier"}}),
            "momiji-mobility",
            "economic operator",
            "2026-07-14T09:05:00Z",
        ),
        (
            "correction",
            serde_json::json!({"Correction": {"field": "de.jp.type-approval-mark", "prior_value": "", "new_value": "jp-epac-type-2027", "reason": "JP road-traffic EPAC type approval"}}),
            "momiji-type-approval",
            "issuing authority",
            "2026-07-14T09:10:00Z",
        ),
        (
            "milestone.record",
            serde_json::json!({"MilestoneRecord": {"counters": {"de.dpp.reparability-score": "8.1", "de.dpp.carbon-footprint": "96.4", "battery.capacity-kwh": "0.72"}}}),
            "e8-bms-controller",
            "device",
            &now,
        ),
    ];
    let have = get(ISSUER, &format!("/passports/{enc}"))
        .map(|r| parse(&r.body)["events"].as_u64().unwrap_or(0) as usize)
        .unwrap_or(0);
    for (i, (type_token, data, actor, role, at)) in events.iter().enumerate().skip(have) {
        let body = serde_json::json!({
            "type": type_token,
            "data": data,
            "actor": actor,
            "actor_role": role,
            "at": at,
        });
        let resp = expect_created(
            &format!("POST /passports/{{urn}}/events ({type_token})"),
            post(
                ISSUER,
                &format!("/passports/{enc}/events"),
                &body.to_string(),
            )
            .expect("events answered"),
            &[201],
        );
        println!(
            "  event {}/{}: {} ({})",
            i + 1,
            events.len(),
            type_token,
            text(&parse(&resp.body), &["event_type"])
        );
    }
    println!(
        "  log: {}/{} events (as-of-reconstructable)",
        events.len(),
        events.len()
    );

    // The full passport view: demo artifact + the projector's store copy.
    let view = get(ISSUER, &format!("/passports/{enc}"))
        .unwrap_or_else(|| die("cannot fetch the passport view"));
    write_artifact(&dir, "02-passport-view.json", &view.body);
    std::fs::write(pilot.dir.join("passports/pilot-e8.json"), &view.body).unwrap();
    let doc = parse(&view.body);
    let log_head = text(&doc, &["log_head"]);
    println!(
        "  view: status {} · log_head {}…",
        text(&doc, &["status"]),
        log_head.chars().take(16).collect::<String>()
    );

    // [6] Tier-A pack + CLI verification against the issuer's keyring.
    println!("== [6] Tier-A pack + CLI verify (anchor from the issuer /keyring)");
    let pack = expect_created(
        "POST /passports/{urn}/pack",
        post(ISSUER, &format!("/passports/{enc}/pack"), "{}").expect("pack answered"),
        &[201],
    );
    write_artifact(&dir, "03-tier-a-pack.json", &pack.body);
    let keyring = get(ISSUER, "/keyring").unwrap_or_else(|| die("cannot fetch the keyring"));
    write_artifact(&dir, "04-issuer-keyring.json", &keyring.body);
    let pack_doc = parse(&pack.body);
    let anchor = text(&parse(&keyring.body), &["roles", "pack", "public"]);
    std::fs::write(dir.join("pack.hex"), text(&pack_doc, &["pack"])).unwrap();
    println!(
        "  pack: {} bytes, QR v{}, anchor {}… (ECDSA-P256)",
        text(&pack_doc, &["bytes"]),
        text(&pack_doc, &["qr_version"]),
        anchor.chars().take(16).collect::<String>()
    );
    // Archival semantics: --max-age 0 — the pack verifies as a
    // document, never stale; freshness is the e2e demo's pinned
    // as-of moments.
    let out = Command::new(&cli)
        .args([
            "verify",
            dir.join("pack.hex").to_str().unwrap(),
            "--anchor",
            &anchor,
            "--max-age",
            "0",
            "--json",
        ])
        .output()
        .unwrap_or_else(|e| die(&format!("CLI verify: {e}")));
    let code = out.status.code().unwrap_or(2);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut doc = serde_json::from_str::<serde_json::Value>(&stdout)
        .unwrap_or(serde_json::json!({ "raw": stdout }));
    doc["exit_code"] = serde_json::json!(code);
    let pretty = serde_json::to_string_pretty(&doc).unwrap();
    write_artifact(&dir, "05-cli-verify.json", &pretty);
    if code != 0 {
        let grade = doc
            .get("verdict")
            .or_else(|| doc.get("grade"))
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        die(&format!(
            "CLI verify graded '{grade}' (exit {code}), expected PASS (0)"
        ));
    }
    println!("  verified: PASS (exit 0) — the three readings named, coverage complete");

    // [7] notarized Tier-C snapshot.
    println!("== [7] notarized Tier-C snapshot (anchored in the transparency log)");
    let view_bytes = std::fs::read(dir.join("02-passport-view.json")).unwrap();
    let state_hash = sha256_hex(&view_bytes);
    let snapshot = serde_json::json!({
        "passport_id": DEMO_URN,
        "state_hash": state_hash,
        "log_head": log_head,
        "submitter": "urn:unidpp:actor:pilot-operator",
        "state_size": 0,
    });
    let snap = expect_created(
        "POST /snapshots",
        post(ARCHIVE, "/snapshots", &snapshot.to_string()).expect("archive answered"),
        &[201],
    );
    write_artifact(&dir, "06-archive-snapshot.json", &snap.body);
    let snap_doc = parse(&snap.body);
    println!(
        "  snapshot {} notarized, anchoring: {}",
        text(&snap_doc, &["snapshot_id"]),
        text(&snap_doc, &["oais", "provenance", "anchoring", "status"])
    );

    // [8] projector two-lens views.
    println!("== [8] projector two-lens views (at {VIEW_AT})");
    for (lens, tag) in [(EU_LENS, "eu"), (JP_LENS, "jp")] {
        let path = format!(
            "/view?passport={}&profile={}&actor=importer&at={VIEW_AT}",
            encode(DEMO_URN),
            encode(lens)
        );
        let view = get(PROJECTOR, &path)
            .unwrap_or_else(|| die(&format!("projector view failed for {lens}")));
        write_artifact(&dir, &format!("07-projector-view-{tag}.json"), &view.body);
        let doc = parse(&view.body);
        println!(
            "  {tag} view: source {}, coverage {}/{} ({})",
            text(&doc, &["profile", "source"]),
            text(&doc, &["coverage", "elements_present"]),
            text(&doc, &["coverage", "elements_required"]),
            text(&doc, &["coverage", "complete"])
        );
    }

    // [9] gateway renders.
    println!("== [9] gateway renders (UNTP triad + EN 18222)");
    let untp = get(GATEWAY, &format!("/untp/product/{}", encode(DEMO_URN)))
        .unwrap_or_else(|| die("UNTP render failed"));
    write_artifact(&dir, "08-gateway-untp.json", &untp.body);
    let doc = parse(&untp.body);
    println!(
        "  UNTP triad: source {}, outcome {} ({} signatures verified)",
        text(&doc, &["rendering", "source"]),
        text(&doc, &["verdict", "outcome"]),
        text(&doc, &["verdict", "coverage", "signatures", "verified"])
    );
    let en = get(
        GATEWAY,
        &format!("/en18222/v1/dppsByProductId/{GTIN_FIXTURE}?representation=full"),
    )
    .unwrap_or_else(|| die("EN 18222 render failed"));
    write_artifact(&dir, "09-gateway-en18222.json", &en.body);
    println!(
        "  EN 18222 render: dpp {} via GTIN {GTIN_FIXTURE}",
        text(&parse(&en.body), &["digitalProductPassportId"])
    );

    // [10] the as-of applicability proof.
    println!("== [10] as-of applicability for momiji:e8");
    for (at, name) in [
        ("2027-06-01T00:00:00Z", "10-applicability-2027-06-01.json"),
        ("2028-06-01T00:00:00Z", "11-applicability-2028-06-01.json"),
    ] {
        let resp = get(
            REGISTRY,
            &format!("/applicability?product_type=momiji:e8&at={at}"),
        )
        .unwrap_or_else(|| die("as-of query failed"));
        write_artifact(&dir, name, &resp.body);
        println!(
            "  at {}: {}",
            at.split('T').next().unwrap_or(at),
            sorted_profiles(&parse(&resp.body)).join(", ")
        );
    }

    println!();
    println!("== demo artifacts:");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json") || n.ends_with(".hex"))
        .collect();
    names.sort();
    for name in names {
        println!("   demo/{name}");
    }
    println!("== seed complete — the stack stays up (unidpp-stack status, unidpp-stack down)");
}

fn sorted_json_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

fn now_stamp() -> String {
    // RFC 3339 UTC from the system clock without a chrono dependency:
    // `date -u` is the family's clock seed pattern (the e2e demo's
    // CLOCK_SEED beat uses the same tool).
    let out = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .expect("clock");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
