use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::PathBuf;
use w3bs_publish::{Expiry, InstallerOptions, ReleaseSpec, Workspace, parse_curl_command, render_installer};
use w3bs_schema::manifest::{ArtifactFormat, CatalogEntry};

/// Publisher tooling for W3BS Package Manager (OpenPkg) repositories.
///
/// Typical flow:  init -> add (per release) -> build -> upload `repo/` -> installer
#[derive(Parser)]
#[command(name = "w3bs-publish", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a repository workspace: one ed25519 key per TUF role and a signed root.
    Init {
        workspace: PathBuf,
        #[arg(long, default_value_t = 365)]
        root_expires_days: i64,
    },
    /// Print the root fingerprint users will see on first-use trust.
    Fingerprint { workspace: PathBuf },
    /// Add one immutable release to the staging tree.
    Add {
        workspace: PathBuf,
        /// Publisher-qualified id, e.g. tools.example/atlas
        #[arg(long)]
        id: String,
        #[arg(long)]
        version: semver::Version,
        #[arg(long, default_value = "stable")]
        channel: String,
        /// PLATFORM=FILE, repeatable (e.g. x86_64-unknown-linux-gnu=dist/atlas.tar.gz)
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<String>,
        /// COMMAND=RELATIVE_PATH inside the artifact, repeatable (e.g. atlas=bin/atlas)
        #[arg(long = "command", required = true)]
        commands: Vec<String>,
        #[arg(long, default_value = "tar.gz")]
        format: String,
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long, default_value = "")]
        license: String,
        /// semver requirement on the W3BS manager, e.g. ">=0.1.0, <0.2.0"
        #[arg(long)]
        manager: Option<String>,
    },
    /// Replace the curl catalog from a JSON file (array of entries, or catalog.json seed rows).
    Catalog { workspace: PathBuf, file: PathBuf },
    /// Revoke a release's artifacts (clients refuse new activations and warn when installed).
    Revoke {
        workspace: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        version: semver::Version,
        #[arg(long)]
        reason: String,
    },
    /// Generate indexes and sign everything into `repo/`. Re-run to refresh expiring metadata.
    Build {
        workspace: PathBuf,
        #[arg(long, default_value_t = 365)]
        targets_days: i64,
        #[arg(long, default_value_t = 90)]
        snapshot_days: i64,
        #[arg(long, default_value_t = 30)]
        timestamp_days: i64,
    },
    /// Generate a curl installer for a package (or the manager itself when --package is omitted).
    Installer {
        #[arg(long)]
        package: Option<String>,
        #[arg(long, default_value = "w3bs")]
        repo_alias: String,
        #[arg(long)]
        repo_url: String,
        #[arg(long)]
        repo_fingerprint: String,
        #[arg(long, default_value = "https://w3bs.org/repo/")]
        manager_repo_url: String,
        #[arg(long)]
        manager_fingerprint: String,
        #[arg(long, default_value = "https://github.com/profullstack/w3bs.org/releases/download")]
        manager_download_base: String,
        #[arg(long)]
        manager_version: String,
        /// PLATFORM=SHA256 of each manager bootstrap archive, repeatable
        #[arg(long = "manager-sha256")]
        manager_sha256: Vec<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn kv(s: &str) -> Result<(String, String)> {
    let (k, v) = s.split_once('=').with_context(|| format!("expected KEY=VALUE, got `{s}`"))?;
    Ok((k.to_owned(), v.to_owned()))
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Init { workspace, root_expires_days } => {
            let fp = Workspace::open(workspace).init(root_expires_days).await?;
            println!("{fp}");
            eprintln!("Keep keys/ secret and backed up; publish only repo/.");
        }
        Cmd::Fingerprint { workspace } => println!("{}", Workspace::open(workspace).fingerprint()?),
        Cmd::Add { workspace, id, version, channel, artifacts, commands, format, description, license, manager } => {
            let format = match format.as_str() {
                "tar.gz" => ArtifactFormat::TarGz,
                "binary" => ArtifactFormat::Binary,
                f => bail!("unknown format `{f}` (tar.gz or binary)"),
            };
            let spec = ReleaseSpec {
                id: id.parse()?,
                version,
                channel,
                description,
                license,
                format,
                artifacts: artifacts.iter().map(|a| kv(a).map(|(k, v)| (k, PathBuf::from(v)))).collect::<Result<_>>()?,
                commands: commands.iter().map(|c| kv(c)).collect::<Result<BTreeMap<_, _>>>()?,
                manager_requirement: manager,
            };
            println!("{}", Workspace::open(workspace).add_release(spec)?.display());
        }
        Cmd::Catalog { workspace, file } => {
            let raw: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&file)?)?;
            let mut entries = vec![];
            for v in raw {
                // Accept either schema entries or seed rows whose `install` is a pasted command.
                if let Some(cmd) = v.get("install").and_then(|i| i.as_str()) {
                    let install = parse_curl_command(cmd).with_context(|| format!("{}: not a curl | sh command: {cmd}", v["id"]))?;
                    let mut e = v.clone();
                    e["install"] = serde_json::to_value(install)?;
                    // Seed rows carry a display name; OpenPkg `name` is the id's name.
                    let id_name = v["id"].as_str().and_then(|i| i.split_once('/')).map(|(_, n)| n.to_owned()).unwrap_or_default();
                    if e["name"].as_str() != Some(id_name.as_str()) {
                        e["title"] = e["name"].clone();
                        e["name"] = id_name.into();
                    }
                    entries.push(serde_json::from_value::<CatalogEntry>(e)?);
                } else {
                    entries.push(serde_json::from_value::<CatalogEntry>(v)?);
                }
            }
            println!("{} catalog entries", Workspace::open(workspace).set_catalog(entries)?);
        }
        Cmd::Revoke { workspace, id, version, reason } => {
            let n = Workspace::open(workspace).revoke(&id.parse()?, &version, &reason)?;
            println!("{n} revoked artifact digest(s) for {id}");
        }
        Cmd::Build { workspace, targets_days, snapshot_days, timestamp_days } => {
            let r = Workspace::open(workspace).build(Expiry { targets: targets_days, snapshot: snapshot_days, timestamp: timestamp_days }).await?;
            println!("signed {} targets, timestamp v{}, root {}", r.targets, r.timestamp_version, r.fingerprint);
        }
        Cmd::Installer { package, repo_alias, repo_url, repo_fingerprint, manager_repo_url, manager_fingerprint, manager_download_base, manager_version, manager_sha256, output } => {
            let s = render_installer(&InstallerOptions {
                package,
                repo_alias,
                repo_url,
                repo_fingerprint,
                manager_repo_url,
                manager_fingerprint,
                manager_download_base,
                manager_version,
                manager_sha256s: manager_sha256.iter().map(|m| kv(m)).collect::<Result<_>>()?,
            });
            match output {
                Some(p) => std::fs::write(p, s)?,
                None => print!("{s}"),
            }
        }
    }
    Ok(())
}
