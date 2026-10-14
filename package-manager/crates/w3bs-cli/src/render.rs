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
        if r.result == Overall::ApprovalRequired {
            eprintln!("hint: re-run interactively, or add --yes (and a pre-provisioned trust root for new repositories)");
        }
        return;
    }
    match r.operation.as_str() {
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
                        let ver = if p["kind"] == "curl" { "(curl)".into() } else { s(&p["version"]) };
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
                    vec![p.id.clone(), p.from.clone().unwrap_or("-".into()), p.to.clone().unwrap_or("-".into()), result]
                })
                .collect(),
        );
    } else if matches!(r.operation.as_str(), "update" | "outdated") {
        println!("Everything is up to date.");
    }
}
