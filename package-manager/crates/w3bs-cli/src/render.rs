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
        eprintln!("error: {} ({})", e.message, e.code);
        if r.result == Overall::ApprovalRequired {
            eprintln!("hint: re-run interactively, or add --yes (and a pre-provisioned trust root for new repositories)");
        }
        return;
    }
    match r.operation.as_str() {
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
