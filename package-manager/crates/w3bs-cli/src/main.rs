//! `w3bs`: W3BS Package Manager.

mod render;

use clap::{Args, Parser, Subcommand};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use w3bs_core::{Engine, InstallRequest, Interact, Layout, Options, RepoAdd, UpdateRequest};
use w3bs_schema::result::{OperationResult, Overall, exit};

const AFTER_HELP: &str = "\
Install from a publisher's website:   curl -fsSL https://<publisher>/install.sh | sh
Install by name:                      w3bs install <package>
Update everything:                    w3bs update

Exit codes: 0 ok (current, pinned and held are not errors), 1 a package operation failed,
2 invalid arguments or ambiguous identity, 3 approval or a trust decision required,
4 global security/state preflight failure, 5 another w3bs holds the lock, 130 interrupted.

Docs: https://w3bs.org/packages   Format: https://logicsrc.com/docs/openpkg";

#[derive(Parser)]
#[command(name = "w3bs", version, about = "W3BS Package Manager: install, inspect, update and remove applications from any publisher", after_help = AFTER_HELP)]
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

#[derive(Subcommand)]
enum Cmd {
    /// Install packages from configured, authenticated repositories
    Install {
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
    },
    /// List installed packages (no network)
    List,
    /// Show an installed package's identity, source, trust, files and history (no network)
    Info { package: String },
    /// Search authenticated repository indexes and catalogs
    Search {
        #[arg(default_value = "")]
        query: String,
        #[arg(long, value_name = "ALIAS")]
        repository: Option<String>,
    },
    /// Show which packages have updates, and which are pinned or held
    Outdated,
    /// Update named packages, or every eligible package when none are named
    Update {
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
    },
    /// Pin a package at its installed version
    Pin {
        package: String,
        #[command(flatten)]
        m: Mutate,
    },
    /// Remove a pin
    Unpin {
        package: String,
        #[command(flatten)]
        m: Mutate,
    },
    /// Re-activate the retained previous version (application files only) and pin it
    Rollback {
        package: String,
        #[command(flatten)]
        m: Mutate,
    },
    /// Remove packages; configuration and user data are kept
    Remove {
        packages: Vec<String>,
        /// Remove every managed application (the manager stays)
        #[arg(long, conflicts_with = "packages")]
        all: bool,
        #[command(flatten)]
        m: Mutate,
    },
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
    /// Manage repositories (sources)
    #[command(subcommand)]
    Repo(RepoCmd),
    /// Manage W3BS Package Manager itself
    #[command(name = "self", subcommand)]
    SelfCmd(SelfCmd),
}

#[derive(Subcommand)]
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

#[derive(Subcommand)]
enum SelfCmd {
    /// Update the manager through its own signed repository
    Update {
        #[command(flatten)]
        m: Mutate,
    },
    /// Remove the manager (refuses while applications remain)
    Remove {
        #[command(flatten)]
        m: Mutate,
    },
    /// Bootstrap: trust the manager repository and install the manager from it
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
        if !self.out.quiet && !self.out.json || self.out.verbose {
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

/// After updating the manager, continue the same command with the new binary, once.
fn handoff() {
    #[cfg(unix)]
    if let Ok(layout) = Layout::from_env() {
        use std::os::unix::process::CommandExt;
        let new = layout.package_dir("w3bs-manager").join("current/bin/w3bs");
        eprintln!("w3bs: continuing with the updated manager");
        let err = std::process::Command::new(new).args(std::env::args_os().skip(1)).env("W3BS_HANDOFF", "1").exec();
        eprintln!("w3bs: could not hand off to the updated manager: {err}");
        std::process::exit(exit::PREFLIGHT);
    }
}

async fn run(cli: Cli) -> (&'static str, w3bs_core::Result<OperationResult>) {
    let layout = match Layout::from_env() {
        Ok(l) => l,
        Err(e) => return ("startup", Err(e)),
    };
    let out = cli.out.clone();
    let mk = |m: &Mutate| {
        Engine::new(
            layout.clone(),
            Options { yes: m.yes, dry_run: m.dry_run, non_interactive: out.non_interactive || out.json },
            Box::new(Tty { out: out.clone() }),
        )
    };
    let ro = Mutate::default();
    match cli.cmd {
        Cmd::Install { packages, version, channel, repository, manifest, pin, origin, m } => {
            let mut e = mk(&m);
            ("install", e.install(InstallRequest { packages, manifest, repository, version, channel, pin, origin: Some(origin) }).await)
        }
        Cmd::List => ("list", mk(&ro).list()),
        Cmd::Info { package } => ("info", mk(&ro).info(&package)),
        Cmd::Search { query, repository } => ("search", mk(&ro).search(&query, repository.as_deref()).await),
        Cmd::Outdated => ("outdated", mk(&ro).outdated().await),
        Cmd::Update { packages, all, version, channel, allow_major, m } => {
            ("update", mk(&m).update(UpdateRequest { packages, all, version, channel, allow_major }).await)
        }
        Cmd::Pin { package, m } => ("pin", mk(&m).pin(&package, true)),
        Cmd::Unpin { package, m } => ("unpin", mk(&m).pin(&package, false)),
        Cmd::Rollback { package, m } => ("rollback", mk(&m).rollback(&package)),
        Cmd::Remove { packages, all, m } => ("remove", mk(&m).remove(&packages, all)),
        Cmd::Doctor => ("doctor", mk(&ro).doctor()),
        Cmd::Repair { m } => ("repair", mk(&m).repair()),
        Cmd::Gc { m } => ("gc", mk(&m).gc()),
        Cmd::Repo(r) => match r {
            RepoCmd::Add { alias, url, root, root_fingerprint, m } => {
                ("repo.add", mk(&m).repo_add(RepoAdd { alias, url, root_file: root, root_fingerprint }).await)
            }
            RepoCmd::List => ("repo.list", mk(&ro).repo_list()),
            RepoCmd::Info { alias } => ("repo.info", mk(&ro).repo_info(&alias)),
            RepoCmd::Update { alias, dry_run } => ("repo.update", mk(&Mutate { yes: false, dry_run }).repo_update(alias.as_deref()).await),
            RepoCmd::Remove { alias, m } => ("repo.remove", mk(&m).repo_remove(&alias)),
        },
        Cmd::SelfCmd(s) => match s {
            SelfCmd::Update { m } => ("self.update", mk(&m).self_update().await),
            SelfCmd::Remove { m } => ("self.remove", mk(&m).self_remove()),
            SelfCmd::Install { repository_url, root_fingerprint, root, m } => ("self.install", mk(&m).self_install(&repository_url, root_fingerprint, root).await),
        },
    }
}
