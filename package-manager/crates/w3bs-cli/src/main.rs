//! `w3bs`: the W3BS command line. One binary for every W3BS surface; the package
//! manager is the first app, W3BS resources (the w3bs:// registry) the second.

mod render;

use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use w3bs_core::resources::{self, Registry};
use w3bs_core::{Engine, InstallRequest, Interact, Layout, Options, RepoAdd, UpdateRequest};
use w3bs_schema::result::{OperationResult, Overall, exit};

const BEFORE_HELP: &str = "\
W3BS: the open Web for humans, agents and devices. Like Nostr but for everything.
One command for every W3BS surface.

Apps:
  packages   w3bs packages            the package catalog (same list as w3bs.org/packages)
             w3bs install <pkg>       install, then `w3bs update` keeps everything current
             w3bs pkg --help          the whole package manager
  resources  w3bs resolve <w3bs://…>  resolve, inspect, verify and run signed W3BS resources
             w3bs resource --help     the whole resource client (search, publish, fork, sign…)";

const AFTER_HELP: &str = "\
Every command takes --json for one versioned result object on stdout.
Exit codes: 0 ok (current, pinned and held are not errors), 1 an operation failed,
2 invalid arguments, 3 approval or a trust decision required, 4 security/state preflight
failure, 5 another w3bs holds the lock, 130 interrupted.

Install or upgrade the CLI:  curl -fsSL https://w3bs.org/install.sh | sh
Docs: https://w3bs.org/packages   Package format: https://logicsrc.com/docs/openpkg";

#[derive(Parser)]
#[command(name = "w3bs", version, about = "W3BS: one command line for the open Web. Packages, resources and more.", before_help = BEFORE_HELP, after_help = AFTER_HELP)]
struct Cli {
    #[command(flatten)]
    out: Output,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone, Default)]
struct Output {
    /// Emit exactly one versioned JSON result object on stdout; never prompt
    #[arg(long, global = true)]
    json: bool,
    /// Never prompt; a needed decision exits with code 3
    #[arg(long, global = true)]
    non_interactive: bool,
    /// More diagnostics on stderr
    #[arg(short, long, global = true, conflicts_with = "quiet")]
    verbose: bool,
    /// Suppress routine progress (not errors or requested data)
    #[arg(short, long, global = true)]
    quiet: bool,
}

#[derive(Args, Clone, Default)]
struct Mutate {
    /// Approve the planned operation (never accepts an unknown trust root or bypasses verification)
    #[arg(short = 'y', long)]
    yes: bool,
    /// Show the plan without changing anything
    #[arg(short = 'n', long)]
    dry_run: bool,
}

// ------------------------------------------------------------------ packages

#[derive(Args, Clone)]
struct CatalogArgs {
    /// Filter by name, vendor, command or description
    #[arg(default_value = "")]
    query: String,
    /// Only agentic tools (coding agents, MCP servers, agent CLIs)
    #[arg(long)]
    agentic: bool,
    /// Only Profullstack's own tools
    #[arg(long)]
    ours: bool,
    /// Include descriptions
    #[arg(short, long)]
    long: bool,
}

#[derive(Args, Clone)]
struct InstallArgs {
    /// Package identities (`publisher.example/name`) or unambiguous short names
    packages: Vec<String>,
    /// Install the exact release on a fresh installation
    #[arg(long, value_name = "SEMVER")]
    version: Option<semver::Version>,
    /// Release channel (default: stable)
    #[arg(long)]
    channel: Option<String>,
    /// Resolve only in this configured repository
    #[arg(long, value_name = "ALIAS")]
    repository: Option<String>,
    /// Install one authenticated manifest target by URL
    #[arg(long, value_name = "HTTPS-URL", conflicts_with_all = ["packages", "version", "channel"])]
    manifest: Option<String>,
    /// Pin the installed release
    #[arg(long)]
    pin: bool,
    /// Diagnostic origin hint recorded in the receipt (set by curl installers)
    #[arg(long, hide = true, default_value = "native")]
    origin: String,
    #[command(flatten)]
    m: Mutate,
}

#[derive(Args, Clone)]
struct UpdateArgs {
    packages: Vec<String>,
    /// Select every eligible package (same as naming none)
    #[arg(long, conflicts_with = "packages")]
    all: bool,
    /// Update one package to this exact newer release
    #[arg(long, value_name = "SEMVER")]
    version: Option<semver::Version>,
    /// Move one package to another channel
    #[arg(long)]
    channel: Option<String>,
    /// Permit incompatible (major, or 0.x minor) stable updates; pins still hold
    #[arg(long)]
    allow_major: bool,
    #[command(flatten)]
    m: Mutate,
}

#[derive(Args, Clone)]
struct SearchArgs {
    #[arg(default_value = "")]
    query: String,
    #[arg(long, value_name = "ALIAS")]
    repository: Option<String>,
}

#[derive(Args, Clone)]
struct OnePackage {
    package: String,
    #[command(flatten)]
    m: Mutate,
}

#[derive(Args, Clone)]
struct RemoveArgs {
    packages: Vec<String>,
    /// Remove every managed application (the CLI itself stays)
    #[arg(long, conflicts_with = "packages")]
    all: bool,
    #[command(flatten)]
    m: Mutate,
}

#[derive(Subcommand, Clone)]
enum PkgCmd {
    /// The package catalog: every tool you can install (same list as w3bs.org/packages)
    Catalog(CatalogArgs),
    /// Install packages
    Install(InstallArgs),
    /// List installed packages (no network)
    List,
    /// Show an installed package's identity, source, trust, files and history (no network)
    Info { package: String },
    /// Search repository indexes and catalogs
    Search(SearchArgs),
    /// Show which packages have updates, and which are pinned or held
    Outdated,
    /// Update named packages, or every eligible package when none are named
    Update(UpdateArgs),
    /// Pin a package at its installed version
    Pin(OnePackage),
    /// Remove a pin
    Unpin(OnePackage),
    /// Re-activate the retained previous version (application files only) and pin it
    Rollback(OnePackage),
    /// Remove packages; configuration and user data are kept
    Remove(RemoveArgs),
    /// Check state, trust, files and environment without repairing anything
    Doctor,
    /// Recover interrupted operations from the journal
    Repair {
        #[command(flatten)]
        m: Mutate,
    },
    /// Delete unreferenced cached downloads and obsolete versions
    Gc {
        #[command(flatten)]
        m: Mutate,
    },
    /// Manage package repositories (sources)
    #[command(subcommand)]
    Repo(RepoCmd),
}

#[derive(Subcommand, Clone)]
enum RepoCmd {
    /// Register a repository after first-use trust approval
    Add {
        alias: String,
        url: String,
        /// Operator-provisioned initial root metadata (headless trust)
        #[arg(long, value_name = "ROOT.JSON")]
        root: Option<PathBuf>,
        /// Expected root fingerprint (`sha256:<hex>`)
        #[arg(long, value_name = "FINGERPRINT")]
        root_fingerprint: Option<String>,
        #[command(flatten)]
        m: Mutate,
    },
    /// List repositories (no network)
    List,
    /// Show a repository (no network)
    Info { alias: String },
    /// Refresh and verify repository metadata
    Update {
        alias: Option<String>,
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
    /// Remove an unused repository registration
    Remove {
        alias: String,
        #[command(flatten)]
        m: Mutate,
    },
}

// ------------------------------------------------------------------ resources

#[derive(Args, Clone)]
struct Api {
    /// Registry origin (default: $W3BS_API or https://w3bs.org)
    #[arg(long, value_name = "ORIGIN")]
    api: Option<String>,
}

#[derive(Subcommand, Clone)]
enum ResCmd {
    /// Search the registry for signed, versioned resources
    Search {
        #[arg(default_value = "")]
        query: String,
        #[arg(long = "type", value_name = "TYPE")]
        kind: Option<String>,
        #[command(flatten)]
        a: Api,
    },
    /// Resolve a w3bs:// URI to its canonical manifest and verification state
    Resolve {
        uri: String,
        #[command(flatten)]
        a: Api,
    },
    /// Inspect identity, provenance, permissions and the canonical artifact
    Inspect {
        uri: String,
        #[command(flatten)]
        a: Api,
    },
    /// Verify a resource's signature, publisher key and revocation state (exit 1 if invalid)
    Verify {
        uri: String,
        #[command(flatten)]
        a: Api,
    },
    /// Protocol fixture checks for a resource
    Conformance {
        uri: String,
        #[command(flatten)]
        a: Api,
    },
    /// Render a prompt template with explicit consent (no model or tool is called)
    Run {
        uri: String,
        /// JSON file of template inputs
        #[arg(long, value_name = "FILE")]
        inputs: Option<PathBuf>,
        /// Authorize rendering
        #[arg(long)]
        consent: bool,
        #[command(flatten)]
        a: Api,
    },
    /// Publish a signed manifest (needs W3BS_PUBLISH_TOKEN)
    Publish {
        manifest: PathBuf,
        #[command(flatten)]
        a: Api,
    },
    /// Revoke an exact resource version (needs W3BS_PUBLISH_TOKEN)
    Revoke {
        uri: String,
        #[arg(long)]
        reason: String,
        #[command(flatten)]
        a: Api,
    },
    /// Save a verified manifest to a file (does not execute anything)
    #[command(alias = "install")]
    Save {
        uri: String,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        #[command(flatten)]
        a: Api,
    },
    /// Derive an unsigned draft under your namespace
    Fork {
        uri: String,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        key_id: String,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        #[command(flatten)]
        a: Api,
    },
    /// Draft a new version of a resource
    Version {
        uri: String,
        /// The new version, e.g. 1.1.0
        #[arg(long = "version", value_name = "SEMVER")]
        new_version: String,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        #[command(flatten)]
        a: Api,
    },
    /// Generate a publisher key pair (private.pem + public.json)
    Keygen {
        #[arg(long)]
        namespace: String,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// Sign a manifest locally with an Ed25519 key
    Sign {
        manifest: PathBuf,
        #[arg(long, value_name = "PRIVATE.PEM")]
        key: PathBuf,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
}

#[derive(Subcommand, Clone)]
enum SelfCmd {
    /// Update the w3bs CLI through its own signed repository
    Update {
        #[command(flatten)]
        m: Mutate,
    },
    /// Remove the w3bs CLI (refuses while installed packages remain)
    Remove {
        #[command(flatten)]
        m: Mutate,
    },
    /// Bootstrap: trust the W3BS repository and install the CLI from it
    Install {
        #[arg(long, default_value = "https://w3bs.org/repo/")]
        repository_url: String,
        #[arg(long)]
        root_fingerprint: Option<String>,
        #[arg(long, value_name = "ROOT.JSON")]
        root: Option<PathBuf>,
        #[command(flatten)]
        m: Mutate,
    },
}

#[derive(Subcommand)]
enum Cmd {
    // Packages: the first app.
    /// List the package catalog (same list as w3bs.org/packages)
    #[command(visible_alias = "catalog")]
    Packages(CatalogArgs),
    /// The package manager: install, list, update, pin, rollback, remove, repositories
    #[command(subcommand)]
    Pkg(PkgCmd),
    /// Install packages (shortcut for `w3bs pkg install`)
    Install(InstallArgs),
    /// Update everything, or the named packages (shortcut for `w3bs pkg update`)
    Update(UpdateArgs),
    /// List installed packages (shortcut for `w3bs pkg list`)
    List,
    /// Search packages (shortcut for `w3bs pkg search`)
    Search(SearchArgs),
    /// Remove packages (shortcut for `w3bs pkg remove`)
    Remove(RemoveArgs),
    #[command(hide = true)]
    Info { package: String },
    #[command(hide = true)]
    Outdated,
    #[command(hide = true)]
    Pin(OnePackage),
    #[command(hide = true)]
    Unpin(OnePackage),
    #[command(hide = true)]
    Rollback(OnePackage),
    #[command(hide = true)]
    Repair {
        #[command(flatten)]
        m: Mutate,
    },
    #[command(hide = true)]
    Gc {
        #[command(flatten)]
        m: Mutate,
    },
    #[command(subcommand, hide = true)]
    Repo(RepoCmd),

    // Resources: the second app.
    /// W3BS resources: search, resolve, inspect, verify, run, publish, fork, sign
    #[command(subcommand, visible_alias = "res")]
    Resource(ResCmd),
    /// Resolve a w3bs:// URI (shortcut for `w3bs resource resolve`)
    Resolve {
        uri: String,
        #[command(flatten)]
        a: Api,
    },
    /// Verify a w3bs:// resource (shortcut for `w3bs resource verify`)
    Verify {
        uri: String,
        #[command(flatten)]
        a: Api,
    },
    #[command(hide = true)]
    Inspect {
        uri: String,
        #[command(flatten)]
        a: Api,
    },

    // The CLI itself.
    /// Check this machine's W3BS state, trust and environment
    Doctor,
    /// Manage the w3bs CLI itself
    #[command(name = "self", subcommand)]
    SelfCmd(SelfCmd),
}

impl Cmd {
    /// Shortcuts map onto the package manager.
    fn into_pkg(self) -> Result<PkgCmd, Cmd> {
        Ok(match self {
            Cmd::Packages(a) => PkgCmd::Catalog(a),
            Cmd::Pkg(p) => p,
            Cmd::Install(a) => PkgCmd::Install(a),
            Cmd::Update(a) => PkgCmd::Update(a),
            Cmd::List => PkgCmd::List,
            Cmd::Search(a) => PkgCmd::Search(a),
            Cmd::Remove(a) => PkgCmd::Remove(a),
            Cmd::Info { package } => PkgCmd::Info { package },
            Cmd::Outdated => PkgCmd::Outdated,
            Cmd::Pin(a) => PkgCmd::Pin(a),
            Cmd::Unpin(a) => PkgCmd::Unpin(a),
            Cmd::Rollback(a) => PkgCmd::Rollback(a),
            Cmd::Repair { m } => PkgCmd::Repair { m },
            Cmd::Gc { m } => PkgCmd::Gc { m },
            Cmd::Repo(r) => PkgCmd::Repo(r),
            Cmd::Doctor => PkgCmd::Doctor,
            other => return Err(other),
        })
    }
}

struct Tty {
    out: Output,
}

impl Interact for Tty {
    fn confirm(&self, prompt: &str) -> Option<bool> {
        if self.out.json || self.out.non_interactive {
            return None;
        }
        let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;
        let mut w = tty.try_clone().ok()?;
        let _ = write!(w, "{prompt} [y/N] ");
        let _ = w.flush();
        let mut line = String::new();
        std::io::BufReader::new(tty).read_line(&mut line).ok()?;
        Some(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
    }
    fn progress(&self, msg: &str) {
        if (!self.out.quiet && !self.out.json) || self.out.verbose {
            eprintln!("w3bs: {msg}");
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let out = cli.out.clone();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let (op, res) = rt.block_on(run(cli));
    let res = res.unwrap_or_else(|e| w3bs_core::engine::failed(op, &e));
    if out.json {
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        render::human(&res);
    }
    let code = res.result.exit_code();
    if res.result == Overall::Ok && res.data.get("handoff").and_then(|h| h.as_bool()) == Some(true) {
        handoff();
    }
    std::process::exit(code);
}

/// After updating the CLI itself, continue the same command with the new binary, once.
fn handoff() {
    #[cfg(unix)]
    if let Ok(layout) = Layout::from_env() {
        use std::os::unix::process::CommandExt;
        let new = layout.package_dir("w3bs-manager").join("current/bin/w3bs");
        eprintln!("w3bs: continuing with the updated w3bs");
        let err = std::process::Command::new(new).args(std::env::args_os().skip(1)).env("W3BS_HANDOFF", "1").exec();
        eprintln!("w3bs: could not hand off to the updated w3bs: {err}");
        std::process::exit(exit::PREFLIGHT);
    }
}

async fn run(cli: Cli) -> (&'static str, w3bs_core::Result<OperationResult>) {
    let out = cli.out.clone();
    match cli.cmd.into_pkg() {
        Ok(p) => run_pkg(p, out).await,
        Err(Cmd::Resource(r)) => run_resource(r).await,
        Err(Cmd::Resolve { uri, a }) => run_resource(ResCmd::Resolve { uri, a }).await,
        Err(Cmd::Verify { uri, a }) => run_resource(ResCmd::Verify { uri, a }).await,
        Err(Cmd::Inspect { uri, a }) => run_resource(ResCmd::Inspect { uri, a }).await,
        Err(Cmd::SelfCmd(s)) => {
            let (layout, mk) = match engine_factory(&out) {
                Ok(x) => x,
                Err(e) => return ("startup", Err(e)),
            };
            let _ = layout;
            match s {
                SelfCmd::Update { m } => ("self.update", mk(&m).self_update().await),
                SelfCmd::Remove { m } => ("self.remove", mk(&m).self_remove()),
                SelfCmd::Install { repository_url, root_fingerprint, root, m } => ("self.install", mk(&m).self_install(&repository_url, root_fingerprint, root).await),
            }
        }
        Err(_) => unreachable!("every command is dispatched"),
    }
}

type Factory = Box<dyn Fn(&Mutate) -> Engine>;

fn engine_factory(out: &Output) -> w3bs_core::Result<(Layout, Factory)> {
    let layout = Layout::from_env()?;
    let l = layout.clone();
    let out = out.clone();
    Ok((
        layout,
        Box::new(move |m: &Mutate| {
            Engine::new(l.clone(), Options { yes: m.yes, dry_run: m.dry_run, non_interactive: out.non_interactive || out.json }, Box::new(Tty { out: out.clone() }))
        }),
    ))
}

async fn run_pkg(cmd: PkgCmd, out: Output) -> (&'static str, w3bs_core::Result<OperationResult>) {
    let (_, mk) = match engine_factory(&out) {
        Ok(x) => x,
        Err(e) => return ("startup", Err(e)),
    };
    let ro = Mutate::default();
    match cmd {
        PkgCmd::Catalog(a) => ("packages", catalog(mk(&ro), a).await),
        PkgCmd::Install(InstallArgs { packages, version, channel, repository, manifest, pin, origin, m }) => {
            ("install", mk(&m).install(InstallRequest { packages, manifest, repository, version, channel, pin, origin: Some(origin) }).await)
        }
        PkgCmd::List => ("list", mk(&ro).list()),
        PkgCmd::Info { package } => ("info", mk(&ro).info(&package)),
        PkgCmd::Search(SearchArgs { query, repository }) => ("search", mk(&ro).search(&query, repository.as_deref()).await),
        PkgCmd::Outdated => ("outdated", mk(&ro).outdated().await),
        PkgCmd::Update(UpdateArgs { packages, all, version, channel, allow_major, m }) => {
            ("update", mk(&m).update(UpdateRequest { packages, all, version, channel, allow_major }).await)
        }
        PkgCmd::Pin(OnePackage { package, m }) => ("pin", mk(&m).pin(&package, true)),
        PkgCmd::Unpin(OnePackage { package, m }) => ("unpin", mk(&m).pin(&package, false)),
        PkgCmd::Rollback(OnePackage { package, m }) => ("rollback", mk(&m).rollback(&package)),
        PkgCmd::Remove(RemoveArgs { packages, all, m }) => ("remove", mk(&m).remove(&packages, all)),
        PkgCmd::Doctor => ("doctor", mk(&ro).doctor()),
        PkgCmd::Repair { m } => ("repair", mk(&m).repair()),
        PkgCmd::Gc { m } => ("gc", mk(&m).gc()),
        PkgCmd::Repo(r) => match r {
            RepoCmd::Add { alias, url, root, root_fingerprint, m } => ("repo.add", mk(&m).repo_add(RepoAdd { alias, url, root_file: root, root_fingerprint }).await),
            RepoCmd::List => ("repo.list", mk(&ro).repo_list()),
            RepoCmd::Info { alias } => ("repo.info", mk(&ro).repo_info(&alias)),
            RepoCmd::Update { alias, dry_run } => ("repo.update", mk(&Mutate { yes: false, dry_run }).repo_update(alias.as_deref()).await),
            RepoCmd::Remove { alias, m } => ("repo.remove", mk(&m).repo_remove(&alias)),
        },
    }
}

/// The catalog, from the signed W3BS repository when this machine trusts one, otherwise
/// from the public listing API (shown as unsigned).
async fn catalog(mut engine: Engine, a: CatalogArgs) -> w3bs_core::Result<OperationResult> {
    let mut res = OperationResult::new("packages");
    let (mut items, source) = match engine.search("", None).await {
        Ok(r) if r.data["complete"] == true && r.data["results"].as_array().is_some_and(|x| x.iter().any(|h| h["kind"] == "curl")) => {
            (r.data["results"].as_array().cloned().unwrap_or_default(), "signed repository")
        }
        _ => {
            let reg = Registry::from_env(None)?;
            let _ = &reg;
            let url = format!("{}/api/packages", std::env::var("W3BS_API").unwrap_or_else(|_| "https://w3bs.org".into()).trim_end_matches('/'));
            let v: Value = reqwest::Client::new()
                .get(&url)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|e| w3bs_core::Error::package("CATALOG_UNAVAILABLE", format!("{url}: {e}")).retryable())?
                .json()
                .await
                .map_err(|e| w3bs_core::Error::package("CATALOG_UNAVAILABLE", e.to_string()))?;
            res.warnings.push("listing fetched from w3bs.org/api/packages (unsigned); run `w3bs self install` to use the signed catalog".into());
            let rows = v["packages"].as_array().cloned().unwrap_or_default();
            let rows = rows
                .into_iter()
                .map(|mut r| {
                    r["title"] = r["name"].clone();
                    r["kind"] = "curl".into();
                    r
                })
                .collect();
            (rows, "w3bs.org/api/packages")
        }
    };
    let q = a.query.to_lowercase();
    items.retain(|h| {
        h["kind"] == "curl"
            && (!a.agentic || h["agentic"] == true)
            && (!a.ours || h["vendor"] == "Profullstack")
            && (q.is_empty() || h.to_string().to_lowercase().contains(&q))
    });
    res.data = json!({ "source": source, "count": items.len(), "long": a.long, "packages": items });
    Ok(res)
}

fn read_json(p: &PathBuf) -> w3bs_core::Result<Value> {
    let s = std::fs::read_to_string(p).map_err(|e| w3bs_core::Error::usage("READ", format!("{}: {e}", p.display())))?;
    serde_json::from_str(&s).map_err(|e| w3bs_core::Error::usage("INVALID_JSON", format!("{}: {e}", p.display())))
}

async fn run_resource(cmd: ResCmd) -> (&'static str, w3bs_core::Result<OperationResult>) {
    let (op, r) = match cmd {
        ResCmd::Search { query, kind, a } => ("resource.search", api_call(&a, "search", json!({ "query": query, "type": kind }).as_object().map(|o| o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())).collect::<serde_json::Map<_, _>>()).map(Value::Object).unwrap()).await),
        ResCmd::Resolve { uri, a } => ("resource.resolve", api_uri(&a, "resolve", &uri).await),
        ResCmd::Inspect { uri, a } => ("resource.inspect", api_uri(&a, "inspect", &uri).await),
        ResCmd::Verify { uri, a } => ("resource.verify", api_uri(&a, "verify", &uri).await),
        ResCmd::Conformance { uri, a } => ("resource.conformance", api_uri(&a, "conformance", &uri).await),
        ResCmd::Run { uri, inputs, consent, a } => {
            let inputs = match inputs.map(|p| read_json(&p)).transpose() {
                Ok(i) => i.unwrap_or_else(|| json!({})),
                Err(e) => return ("resource.run", Err(e)),
            };
            ("resource.run", api_call(&a, "run", json!({ "uri": uri, "inputs": inputs, "consent": consent })).await)
        }
        ResCmd::Publish { manifest, a } => match read_json(&manifest) {
            Ok(m) => ("resource.publish", api_call(&a, "publish", json!({ "manifest": m })).await),
            Err(e) => ("resource.publish", Err(e)),
        },
        ResCmd::Revoke { uri, reason, a } => ("resource.revoke", api_call(&a, "revoke", json!({ "uri": uri, "reason": reason })).await),
        ResCmd::Save { uri, out, a } => ("resource.save", derive(&a, &uri, &out, None).await),
        ResCmd::Fork { uri, namespace, name, key_id, out, a } => ("resource.fork", derive(&a, &uri, &out, Some(Derive::Fork { namespace, name, key_id })).await),
        ResCmd::Version { uri, new_version, out, a } => ("resource.version", derive(&a, &uri, &out, Some(Derive::Version(new_version))).await),
        ResCmd::Keygen { namespace, out } => ("resource.keygen", resources::keygen(&namespace, &out)),
        ResCmd::Sign { manifest, key, out } => (
            "resource.sign",
            (|| {
                let m = read_json(&manifest)?;
                let k = std::fs::read_to_string(&key).map_err(|e| w3bs_core::Error::usage("READ", format!("{}: {e}", key.display())))?;
                let signed = resources::sign_manifest(&m, &k)?;
                resources::write_new(&out, &signed)?;
                Ok(signed)
            })(),
        ),
    };
    let mut res = OperationResult::new(op);
    match r {
        Ok(data) => {
            if op == "resource.verify" && data["verification"]["valid"] != true {
                res.result = Overall::Failure;
            }
            res.data = data;
            (op, Ok(res))
        }
        Err(e) => (op, Err(e)),
    }
}

async fn api_call(a: &Api, operation: &str, args: Value) -> w3bs_core::Result<Value> {
    Registry::from_env(a.api.as_deref())?.call(operation, args).await
}

async fn api_uri(a: &Api, operation: &str, uri: &str) -> w3bs_core::Result<Value> {
    resources::parse_uri(uri)?;
    api_call(a, operation, json!({ "uri": uri })).await
}

enum Derive {
    Fork { namespace: String, name: String, key_id: String },
    Version(String),
}

/// `save` writes the verified manifest; `fork`/`version` write an unsigned draft to sign.
async fn derive(a: &Api, uri: &str, out: &PathBuf, how: Option<Derive>) -> w3bs_core::Result<Value> {
    let resolved = api_uri(a, "resolve", uri).await?;
    if resolved["verification"]["valid"] != true {
        return Err(w3bs_core::Error::package("VERIFICATION_FAILED", "cannot save or derive from a revoked or unverified resource"));
    }
    let mut m = resolved["manifest"].clone();
    if let Some(how) = how {
        let o = m.as_object_mut().unwrap();
        o.remove("proof");
        m["provenance"]["derivedFrom"] = json!([resolved["canonicalUri"]]);
        m["createdAt"] = w3bs_core::now().into();
        let kind = m["type"].as_str().unwrap_or_default().to_owned();
        let (namespace, name, version) = match &how {
            Derive::Fork { namespace, name, .. } => (namespace.clone(), name.clone(), "1.0.0".to_owned()),
            Derive::Version(v) => (m["publisher"]["namespace"].as_str().unwrap_or_default().to_owned(), resources::parse_uri(m["id"].as_str().unwrap_or_default())?.name, v.clone()),
        };
        let id = format!("w3bs://{kind}/{namespace}/{name}@{version}");
        resources::parse_uri(&id)?;
        m["id"] = id.clone().into();
        m["version"] = version.into();
        m["publisher"]["namespace"] = namespace.clone().into();
        if let Derive::Fork { key_id, .. } = &how {
            m["publisher"]["keyId"] = key_id.clone().into();
            m["publisher"]["name"] = namespace.into();
        }
        m["locations"]["canonical"] = format!("https://prompt.w3bs.org/api/manifest?uri={}", url_encode(&id)).into();
        m["locations"]["mirrors"] = json!([]);
    }
    resources::write_new(out, &m)?;
    Ok(json!({ "written": out, "manifest": m }))
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
