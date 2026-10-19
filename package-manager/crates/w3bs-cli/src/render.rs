//! Human-readable output. Data goes to stdout, diagnostics to stderr.

use serde_json::Value;
use w3bs_schema::result::{OperationResult, Overall};

fn s(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn table(headers: &[&str], rows: Vec<Vec<String>>) {
    let mut w: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<String>| {
        let mut out = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                out.push_str(c);
            } else {
                out.push_str(&format!("{c:<width$}  ", width = w[i]));
            }
        }
        println!("{}", out.trim_end());
    };
    line(headers.iter().map(|h| h.to_string()).collect());
    for r in rows {
        line(r);
    }
}

pub fn human(r: &OperationResult) {
    for w in &r.warnings {
        eprintln!("warning: {w}");
    }
    if let Some(e) = &r.error {
        if e.code == "NOT_SIGNED_IN" {
            eprintln!("{}", e.message);
            return;
        }
        eprintln!("error: {} ({})", e.message, e.code);
        if e.code == "PUBLISHER_KEY_CHANGED" {
            eprintln!("hint: check with the publisher that the key change is genuine before using --accept-new-key");
        } else if r.result == Overall::ApprovalRequired && r.operation != "open" && e.code != "NEEDS_ROOT" {
            eprintln!("hint: re-run interactively, or add --yes (and a pre-provisioned trust root for new repositories)");
        }
        return;
    }
    match r.operation.as_str() {
        "publish.key.add" => {
            let d = &r.data;
            let p = &d["proof"];
            println!("Registered {} for {} on {}{}", s(&d["fingerprint"]), s(&d["namespace"]), s(&d["origin"]), if d["agent"] == true { " (signed through ssh-agent)" } else { "" });
            if d["domain_verified"] == true {
                println!("Domain proof found: this key can sign releases for {} now.", s(&d["namespace"]));
            } else {
                println!("\nProve you control {} with either of these, then run: {}\n", s(&d["namespace"]), s(&d["next"]));
                println!("  1. Serve {} containing the line:\n       {}", s(&p["well_known"]), s(&p["line"]));
                println!("  2. Or add a DNS TXT record:\n       {}  TXT  \"{}\"", s(&p["dns"]), s(&p["line"]));
            }
            return;
        }
        "publish.key.ls" | "publish.key.verify" => {
            let keys = r.data["keys"].as_array().cloned().unwrap_or_default();
            if keys.is_empty() {
                println!("No publisher keys yet. Add one: w3bs publish key add <your-domain>");
                return;
            }
            let rows = keys
                .iter()
                .map(|k| {
                    let state = if !k["revoked_at"].is_null() { "revoked" } else if k["domain_verified"] == true { "verified" } else { "domain proof missing" };
                    vec![s(&k["namespace"]), s(&k["fingerprint"]), s(&k["type"]), state.into()]
                })
                .collect();
            table(&["NAMESPACE", "FINGERPRINT", "TYPE", "STATE"], rows);
            return;
        }
        "publish.key.revoke" => {
            println!("Revoked {}. Releases it signed stay listed, and clients refuse to install them.", s(&r.data["fingerprint"]));
            return;
        }
        "publish.release" => {
            let d = &r.data;
            if d["dry_run"] == true {
                println!("{}", serde_json::to_string_pretty(&d["statement"]).unwrap_or_default());
                print!("{}", s(&d["signature"]));
                println!("Signed {} {} with {} (dry run: not submitted)", s(&d["package"]), s(&d["version"]), s(&d["fingerprint"]));
            } else {
                println!("Published {} {} signed by {}", s(&d["package"]), s(&d["version"]), s(&d["fingerprint"]));
                if !d["url"].is_null() {
                    println!("{}", s(&d["url"]));
                }
            }
            return;
        }
        "login" | "whoami" => {
            let u = &r.data["user"];
            let who = match u["email"].as_str() {
                Some(email) if !email.is_empty() => format!("{} ({email})", s(&u["username"])),
                _ => s(&u["username"]),
            };
            if r.operation == "login" {
                println!("Signed in to {} as {who}", s(&r.data["origin"]));
            } else {
                println!("{who} on {}", s(&r.data["origin"]));
            }
            return;
        }
        "logout" => {
            if r.data["signedIn"] == true {
                println!("Signed out of {}", s(&r.data["origin"]));
                if r.data["revoked"] != true {
                    eprintln!("warning: could not reach {} to revoke the session; the local credentials are deleted", s(&r.data["origin"]));
                }
            } else {
                println!("Not signed in to {}", s(&r.data["origin"]));
            }
            return;
        }
        "open" => {
            println!("Opened a terminal window ({}) to confirm installing {}.", s(&r.data["terminal"]), s(&r.data["package"]));
            return;
        }
        "handler.install" | "handler.uninstall" | "handler.status" => {
            let d = &r.data;
            let place = if d["desktop_file"].is_null() { s(&d["app"]) } else { s(&d["desktop_file"]) };
            match r.operation.as_str() {
                "handler.install" => println!("Registered the w3bs:// handler: {place}"),
                "handler.uninstall" if d["removed"] == true => println!("Removed the w3bs:// handler: {place}"),
                "handler.uninstall" => println!("The w3bs:// handler was not installed ({place})."),
                _ => println!("Handler: {}", if d["installed"] == true { place } else { "not installed (run `w3bs handler install`)".into() }),
            }
            if r.operation != "handler.uninstall" {
                if !d["exec"].is_null() {
                    println!("Runs: {} open <link>", s(&d["exec"]).trim_end_matches(" open %u"));
                }
                println!("Default for w3bs:// links: {}", if d["default"].is_null() { "none".into() } else { s(&d["default"]) });
                if d["installed"] == true && d["registered"] != true {
                    println!("warning: another application, or none, is the default for w3bs:// links");
                }
            }
            return;
        }
        "fleet.join" => {
            let d = &r.data;
            println!("Joined {} to your fleet on {}. It reports every {}s; see {}/fleet", s(&d["name"]), s(&d["origin"]), s(&d["interval"]), s(&d["origin"]));
            match d["service"].as_str() {
                Some("system") => println!("Agent: system service w3bs-fleet.service"),
                Some("user") => println!("Agent: user service w3bs-fleet.service (systemctl --user status w3bs-fleet)"),
                _ => {}
            }
            println!("Remote actions: {}", if d["allow_actions"] == true { "allowed (refresh, update, restart-desktop)" } else { "not allowed" });
            if let Some(sc) = d["screen"].as_object() {
                let mode = if sc["mode"] == "control" { "view and control" } else { "view only" };
                println!("Screen: shared ({mode}) from {}; watch it at {}/fleet/{}/screen", s(&sc["vnc"]), s(&d["origin"]), s(&d["machine_id"]));
                if sc["reachable"] != true {
                    eprintln!("warning: nothing answers on {} yet; start a VNC server there, see https://w3bs.org/docs/fleet-screen.md", s(&sc["vnc"]));
                }
            }
            for n in d["notes"].as_array().into_iter().flatten() {
                eprintln!("note: {}", s(n));
            }
            return;
        }
        "fleet.screen" => {
            let d = &r.data;
            let sc = &d["screen"];
            if sc.is_null() {
                println!("Screen sharing is off for {}. Turn it on with: w3bs fleet screen on [--control]", s(&d["name"]));
                return;
            }
            let mode = if sc["mode"] == "control" { "view and control" } else { "view only" };
            println!("Screen sharing is on for {} ({mode}), from the VNC server on {}", s(&d["name"]), s(&sc["vnc"]));
            println!("Watch it at {}", s(&d["url"]));
            if sc["reachable"] != true {
                eprintln!("warning: nothing answers on {} yet; start a VNC server there, see https://w3bs.org/docs/fleet-screen.md", s(&sc["vnc"]));
            }
            return;
        }
        "dist-upgrade" => {
            let sys = &r.data["system"];
            let provider = sys["provider"].as_str().unwrap_or("system");
            match sys["status"].as_str() {
                Some("upgraded") => println!("System packages ({provider}) upgraded."),
                Some("planned") => {
                    println!("Would upgrade the system packages ({provider}):");
                    for c in sys["commands"].as_array().into_iter().flatten() {
                        println!("  {}", s(c));
                    }
                }
                Some("done") => {}
                _ => println!("System packages: skipped."),
            }
            if let Some(n) = sys["note"].as_str() {
                eprintln!("note: {n}");
            }
        }
        "fleet.desktop.start" => {
            let d = &r.data;
            println!("The Kubuntu desktop is running (container {}, {})", s(&d["container"]), s(&d["image"]));
            println!("AI agents: desktop layer MCP at {} (register one: w3bs fleet desktop agent <name>)", s(&d["mcp"]));
            if let Some(url) = d["url"].as_str() {
                let mode = if d["screen"]["mode"] == "control" { "view and control" } else { "view only" };
                println!("Watch it ({mode}): {url}");
            }
            for n in d["notes"].as_array().into_iter().flatten() {
                eprintln!("note: {}", s(n));
            }
            return;
        }
        "fleet.desktop.stop" => {
            let d = &r.data;
            println!("{}", if d["removed"] == true { "Stopped the desktop (its home directory is kept)" } else { "No desktop was running" });
            if d["screen_off"] == true {
                println!("Screen sharing is off");
            }
            return;
        }
        "fleet.desktop.status" => {
            let d = &r.data;
            println!("Desktop: {}{}", s(&d["state"]), d["started_at"].as_str().map(|t| format!(" since {t}")).unwrap_or_default());
            if d["image"].is_string() && d["image"] != d["current_image"] {
                println!("Image: {} (a newer one is built in: w3bs fleet desktop start --rebuild)", s(&d["image"]));
            }
            println!("VNC: {} ({})", s(&d["vnc"]), if d["vnc_ready"] == true { "ready" } else { "not ready" });
            println!("MCP: {}", s(&d["mcp"]));
            println!("Shared: {}", if d["screen"].is_null() { "no".to_owned() } else { format!("{} from {}", s(&d["screen"]["mode"]), s(&d["screen"]["vnc"])) });
            println!("Session logs: {}", s(&d["logs"]));
            return;
        }
        "fleet.desktop.logs" => {
            print!("{}", r.data["log"].as_str().unwrap_or_default());
            eprintln!("session logs: {}", s(&r.data["session_logs"]));
            return;
        }
        "fleet.desktop.agent" => {
            let d = &r.data;
            println!("Registered {} ({}) on the desktop. Add it to Claude Code:", s(&d["agent"]), s(&d["preset"]));
            println!("{}", s(&d["claude_code"]));
            return;
        }
        "fleet.view" => {
            let d = &r.data;
            println!("{}", s(&d["url"]));
            if d["screen"].is_null() {
                eprintln!("note: {} does not share its screen; on it, run: w3bs fleet screen on", s(&d["machine"]));
            } else if d["listening"] != true {
                eprintln!("note: {}'s agent is not connected for screen sharing right now", s(&d["machine"]));
            }
            return;
        }
        "fleet.status" => {
            let d = &r.data;
            println!("{} ({}) on {}", s(&d["name"]), s(&d["machine_id"]), s(&d["origin"]));
            println!("Remote actions: {}", if d["allow_actions"] == true { "allowed" } else { "not allowed" });
            let sc = &d["info"]["screen"];
            println!(
                "Screen: {}",
                if sc.is_null() { "not shared (w3bs fleet screen on)".to_owned() } else { format!("{} from {}{}", if sc["mode"] == "control" { "view and control" } else { "view only" }, s(&sc["vnc"]), if sc["reachable"] == true { "" } else { " (nothing answers there)" }) }
            );
            println!("Agent service: {}", if d["service"].is_null() { "not installed".to_owned() } else { s(&d["service"]) });
            println!("Reports: {}", serde_json::to_string_pretty(&d["info"]).unwrap_or_default());
            return;
        }
        "os.status" => {
            let d = &r.data;
            if d["w3bs_os"] == true {
                println!("W3BS OS {} ({})", s(&d["version"]), s(&d["edition"]));
            } else {
                println!("Not W3BS OS (no {})", crate::os::RELEASE_FILE);
            }
            println!("Alpine: {} on branch {} ({})", s(&d["alpine"]["release"]), s(&d["alpine_branch"]), s(&d["alpine"]["arch"]));
            println!("Newest branch: {}{}", s(&d["newest_branch"]), d["qualified_max"].as_str().map(|m| format!(" (W3BS has qualified up to {m})")).unwrap_or_default());
            let up = &d["upgrade"];
            if let Some(b) = up["available"].as_str() {
                println!("Upgrade available: {b}  (w3bs os upgrade)");
            } else if let Some(h) = up["held"].as_str() {
                println!("Upgrade: {h} is released, held: {}", s(&up["reason"]));
            } else {
                println!("Upgrade: none (on the newest branch)");
            }
            println!("Pending updates: {}{}", s(&d["pending_updates"]), if d["pending_updates"].as_u64().unwrap_or(0) > 0 { "  (w3bs os update)" } else { "" });
            let k = &d["kernel"];
            if k["applies"] == false {
                println!("Kernel: {} (container: the host's kernel)", s(&k["running"]));
            } else {
                println!("Kernel: running {}, installed {}{}", s(&k["running"]), k["installed"].as_array().map(|a| a.iter().map(s).collect::<Vec<_>>().join(", ")).unwrap_or_default(), if d["reboot_needed"] == true { " (reboot needed)" } else { "" });
            }
            if let Some(st) = d["journal"]["state"].as_str() {
                println!("Last upgrade: {} -> {} ({st})", s(&d["journal"]["from"]), s(&d["journal"]["to"]));
            }
            println!("w3bs {}", s(&d["w3bs_version"]));
            return;
        }
        "os.setup" => {
            let d = &r.data;
            if d["dry_run"] == true {
                println!("Would run: {}", s(&d["command_line"]));
            }
        }
        "os.update" | "os.upgrade" => {
            let d = &r.data;
            if let Some(m) = d["message"].as_str() {
                println!("{m}");
                return;
            }
            if d["dry_run"] == true {
                println!("Plan (dry run, nothing changed):");
                for l in d["plan"].as_array().cloned().unwrap_or_default() {
                    println!("  {}", s(&l));
                }
                return;
            }
            for st in d["steps"].as_array().cloned().unwrap_or_default() {
                let detail = match &st["detail"] {
                    Value::Array(a) if a.len() > 3 => format!("{} items", a.len()),
                    Value::Null => String::new(),
                    Value::String(x) if x.is_empty() => String::new(),
                    v => s(v),
                };
                println!("{:<18} {:<8} {detail}", s(&st["step"]), s(&st["status"]));
            }
            if d["action"] == "rollback" {
                println!("Restored {}.", s(&d["restored"]));
                println!("{}", s(&d["next"]));
            } else if r.operation == "os.upgrade" {
                println!("Upgraded {} -> {} (Alpine {}).", s(&d["from"]), s(&d["to"]), s(&d["alpine"]));
            } else {
                println!("Updated (Alpine {}).", s(&d["alpine"]));
            }
            return;
        }
        "ab.ls" => {
            let list = r.data["experiments"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("No experiments yet. Make one: w3bs ab new <id> --variant control --variant b");
                return;
            }
            let rows = list
                .iter()
                .map(|x| {
                    let variants = x["variants"].as_array().map(|v| v.iter().map(|v| s(&v["id"])).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    vec![s(&x["slug"]), s(&x["state"]), s(&x["unit"]), variants, s(&x["name"])]
                })
                .collect();
            table(&["ID", "STATE", "UNIT", "VARIANTS", "NAME"], rows);
            return;
        }
        "ab.new" | "ab.state" => {
            let x = &r.data["experiment"];
            println!("{} is {}. {}/ab-testing/{}", s(&x["slug"]), s(&x["state"]), s(&r.data["origin"]), s(&x["slug"]));
            if x["state"] == "draft" {
                println!("Start it with: w3bs ab start {}", s(&x["slug"]));
            }
            return;
        }
        "ab.show" => {
            let res = &r.data["results"];
            let x = &res["experiment"];
            println!("{} ({}) · {} · unit {} · goal {}", s(&x["name"]), s(&x["slug"]), s(&x["state"]), s(&res["unit"]), s(&x["goal"]));
            let pct = |v: &Value| v.as_f64().map(|f| format!("{:.1}%", f * 100.0)).unwrap_or_else(|| "no data".into());
            let rows = res["variants"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|v| {
                    let ci = v["interval95"].as_array().map(|i| format!("{}-{}", pct(&i[0]), pct(&i[1]))).unwrap_or_default();
                    let vs = if v["vsControl"].is_null() {
                        String::new()
                    } else {
                        let d = v["vsControl"]["difference"].as_f64().unwrap_or(0.0) * 100.0;
                        let p = v["vsControl"]["pValue"].as_f64().map(|p| if p < 0.001 { " p<0.001".into() } else { format!(" p={p:.3}") }).unwrap_or_default();
                        format!("{d:+.1} pts{p}")
                    };
                    vec![s(&v["id"]), s(&v["weight"]), s(&v["assigned"]), s(&v["exposed"]), s(&v["converted"]), pct(&v["conversionRate"]), ci, vs]
                })
                .collect();
            table(&["VARIANT", "WEIGHT", "ASSIGNED", "EXPOSED", "CONVERTED", "RATE", "95%", "VS CONTROL"], rows);
            let f = &res["floors"];
            println!(
                "Floors {}: {} per variant, {}s running (so far {}s).",
                if f["met"] == true { "met" } else { "not met" },
                s(&f["minimumEligiblePerVariant"]),
                s(&f["minimumObservationSeconds"]),
                s(&f["observedSeconds"])
            );
            println!("{}", s(&res["note"]));
            return;
        }
        "ab.assign" => {
            let a = &r.data["assignment"];
            if a["variant"].is_null() {
                println!("{}", s(&a["reason"]));
            } else {
                println!("{} -> {} {}{}", s(&a["participant"]), s(&a["variant"]), a["parameters"], if a["new"] == true { " (new)" } else { "" });
            }
            return;
        }
        "ab.export" => {
            println!("{}", serde_json::to_string_pretty(&r.data["document"]).unwrap_or_default());
            return;
        }
        "ab.rm" => {
            println!("Deleted {}.", s(&r.data["slug"]));
            return;
        }
        "fleet.ls" => {
            let list = r.data["machines"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("No machines yet. Add one on {}/fleet, then run `w3bs fleet join <token>` on it.", s(&r.data["origin"]));
                return;
            }
            let pct = |part: &Value, total: &Value| match (part.as_f64(), total.as_f64()) {
                (Some(a), Some(t)) if t > 0.0 => format!("{:.0}%", 100.0 * (1.0 - a / t)),
                _ => "-".into(),
            };
            let rows = list
                .iter()
                .map(|m| {
                    let i = &m["info"];
                    vec![
                        s(&m["name"]),
                        if m["online"] == true { "yes".into() } else { "no".into() },
                        m["last_seen_at"].as_str().map(crate::fleet::ago).unwrap_or_else(|| "never".into()),
                        s(&i["os"]),
                        i["load"].get(0).and_then(|v| v.as_f64()).map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".into()),
                        pct(&i["mem"]["available"], &i["mem"]["total"]),
                        pct(&i["disk"]["available"], &i["disk"]["total"]),
                        s(&m["id"]),
                    ]
                })
                .collect();
            table(&["NAME", "ONLINE", "LAST SEEN", "OS", "LOAD", "MEM%", "DISK%", "ID"], rows);
            return;
        }
        "fleet.run" => {
            println!("Queued {} on {} ({})", s(&r.data["action"]["action"]), s(&r.data["machine"]), s(&r.data["action"]["id"]));
            return;
        }
        "fleet.rm" => {
            println!("Removed {} from your fleet", s(&r.data["machine"]));
            return;
        }
        "fleet.leave" => {
            println!("{} left the fleet on {}", s(&r.data["name"]), s(&r.data["origin"]));
            if r.data["server"] != true {
                eprintln!("warning: could not reach {}; remove the machine on {}/fleet too", s(&r.data["origin"]), s(&r.data["origin"]));
            }
            return;
        }
        "packages" => {
            let pkgs = r.data["packages"].as_array().cloned().unwrap_or_default();
            if pkgs.is_empty() {
                println!("No matching packages.");
                return;
            }
            let long = r.data["long"] == true;
            table(
                &["PACKAGE", "TYPE", "VENDOR", "INSTALL"],
                pkgs.iter()
                    .map(|p| {
                        let ty = if p["agentic"] == true { "agentic" } else { "cli" };
                        vec![s(&p["id"]), ty.into(), s(&p["vendor"]), s(&p["install"])]
                    })
                    .collect(),
            );
            if long {
                println!();
                for p in &pkgs {
                    println!("{}  {}\n    {}", s(&p["id"]), s(&p["title"]), s(&p["description"]));
                }
            }
            eprintln!("{} packages ({}). Install one with: w3bs install <package>", pkgs.len(), s(&r.data["source"]));
            return;
        }
        "resource.search" => {
            let items = r.data["resources"].as_array().cloned().unwrap_or_default();
            table(
                &["RESOURCE", "VERSION", "VERIFIED", "NAME"],
                items.iter().map(|i| vec![s(&i["id"]), s(&i["version"]), if i["verification"]["valid"] == true { "yes".into() } else { "NO".into() }, s(&i["name"])]).collect(),
            );
            return;
        }
        "resource.resolve" | "resource.inspect" | "resource.verify" => {
            let m = &r.data["manifest"];
            let v = &r.data["verification"];
            if !m.is_null() {
                println!("{}\n  {}", s(&r.data["canonicalUri"]), s(&m["description"]));
                println!("  publisher: {} ({})  license: {}", s(&m["publisher"]["name"]), s(&m["publisher"]["namespace"]), s(&m["license"]));
            } else {
                println!("{}", s(&r.data["canonicalUri"]));
            }
            println!(
                "  verification: {}  signature: {}  publisher trusted: {}  revoked: {}  authority: {}",
                if v["valid"] == true { "valid" } else { "INVALID" },
                s(&v["signatureValid"]),
                s(&v["publisherTrusted"]),
                s(&v["revoked"]),
                s(&v["authority"])
            );
            println!("  digest: {}", s(&v["digest"]));
            return;
        }
        // The model's answer when a runner ran, else the rendered instructions.
        "resource.run" => {
            let d = &r.data;
            println!("{}", s(if d["runner"].is_null() { &d["output"] } else { &d["runner"]["answer"] }));
            return;
        }
        op if op.starts_with("resource.") => {
            println!("{}", serde_json::to_string_pretty(&r.data).unwrap());
            return;
        }
        "list" => {
            let pkgs = r.data["packages"].as_array().cloned().unwrap_or_default();
            if pkgs.is_empty() {
                println!("No packages installed. Find some with `w3bs search`.");
                return;
            }
            table(
                &["PACKAGE", "VERSION", "CHANNEL", "PINNED", "SOURCE", "COMMANDS"],
                pkgs.iter()
                    .map(|p| {
                        let ver = match (p["kind"].as_str(), p["version"].as_str()) {
                            (Some("curl"), Some(v)) => format!("{v} (curl)"),
                            (Some("curl"), None) => "(curl)".into(),
                            _ => s(&p["version"]),
                        };
                        let cmds = p["commands"].as_array().map(|a| a.iter().map(s).collect::<Vec<_>>().join(",")).unwrap_or_default();
                        vec![s(&p["id"]), ver, s(&p["channel"]), s(&p["pinned"]), s(&p["repository"]), cmds]
                    })
                    .collect(),
            );
        }
        "search" => {
            let hits = r.data["results"].as_array().cloned().unwrap_or_default();
            if hits.is_empty() {
                println!("No matches{}.", if r.data["complete"] == false { " in the repositories that could be checked" } else { "" });
            }
            table(
                &["PACKAGE", "KIND", "REPOSITORY", "DESCRIPTION"],
                hits.iter().map(|h| vec![s(&h["id"]), s(&h["kind"]), s(&h["repository"]), s(&h["description"])]).collect(),
            );
        }
        "repo.list" => {
            let repos = r.data["repositories"].as_array().cloned().unwrap_or_default();
            if repos.is_empty() {
                println!("No repositories. Add one with `w3bs repo add <alias> <https-url>`.");
                return;
            }
            table(&["ALIAS", "URL", "FINGERPRINT"], repos.iter().map(|x| vec![s(&x["alias"]), s(&x["url"]), s(&x["root_fingerprint"])]).collect());
        }
        "doctor" => {
            let issues = r.data["issues"].as_array().cloned().unwrap_or_default();
            if issues.is_empty() {
                println!("No problems found. Store: {}", s(&r.data["store"]));
            }
            for i in issues {
                println!("{}: {} ({})", s(&i["severity"]), s(&i["message"]), s(&i["code"]));
            }
        }
        "info" | "repo.info" | "repo.add" | "repo.update" | "repo.remove" | "gc" | "repair" | "self.remove" => {
            if let Value::Object(m) = &r.data {
                for (k, v) in m {
                    match v {
                        Value::Array(a) if !a.is_empty() && a[0].is_object() => {
                            println!("{k}:");
                            for x in a {
                                println!("  {}", x);
                            }
                        }
                        _ => println!("{k}: {}", s(v)),
                    }
                }
            }
        }
        _ => {}
    }
    if !r.packages.is_empty() {
        table(
            &["PACKAGE", "INSTALLED", "CANDIDATE", "RESULT"],
            r.packages
                .iter()
                .map(|p| {
                    let st = serde_json::to_value(p.status).unwrap();
                    let mut result = s(&st).replace('_', " ");
                    if let Some(reason) = &p.reason {
                        result = format!("{result}: {reason}");
                    }
                    if let Some(e) = &p.error {
                        result = format!("{result}: {} ({})", e.message, e.code);
                    }
                    if let Some(sig) = &p.signature {
                        result = format!("{result} [{sig}]");
                    }
                    vec![p.id.clone(), p.from.clone().unwrap_or("-".into()), p.to.clone().unwrap_or("-".into()), result]
                })
                .collect(),
        );
    } else if matches!(r.operation.as_str(), "update" | "outdated" | "dist-upgrade") && r.data.get("manager_update").is_none() {
        println!("Everything is up to date.");
    }
    if let Some(n) = r.data.get("manager_update").and_then(|v| v.as_str()) {
        println!("{n}");
    }
}
