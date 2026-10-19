//! `w3bs`: the W3BS command line. One binary for every W3BS surface; the package
//! manager is the first app, W3BS resources (the w3bs:// registry) the second.

mod ab;
mod auth;
mod desktop;
mod distupgrade;
mod fleet;
mod publish;
mod screen;
mod render;

use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::ffi::OsString;
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
             w3bs resource --help     the whole resource client (search, publish, fork, sign…)
  account    w3bs login               sign in to your W3BS account (opens the browser; works
                                      over SSH: shows a code to enter at w3bs.org/device)
             w3bs whoami / logout     show or end this machine's session
  fleet      w3bs fleet join <token>  add this machine to your fleet on w3bs.org/fleet
             w3bs fleet ls            your machines, online state and health
             w3bs fleet run <m> <a>   queue refresh, update or restart-desktop on a machine
             w3bs fleet screen on     let the fleet owner watch this desktop (VNC, view only)
             w3bs fleet view <m>      watch a machine's desktop in the browser
             w3bs fleet desktop start a headless Kubuntu desktop (Docker) for AI agents, shared there
             w3bs fleet desktop password  ask for a VNC password on its screen page
             w3bs fleet --help        status, agent, leave, rm
  ab         w3bs ab ls               your A/B tests on w3bs.org/ab-testing (OpenABTest)
             w3bs ab new <id> --variant a --variant b   then start, show, pause, close
             w3bs ab --help           assign, resume, export, rm
  publish    w3bs publish key add <domain>   register your SSH key for a publisher namespace
             w3bs publish release --package <ns/name> --version <v> --install \"curl … | sh\"
                                      sign a release with that key (W3BS-SIGN-1)";

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
    /// Reinstall even when the package is already installed and current
    #[arg(long, conflicts_with_all = ["version", "channel", "pin", "manifest"])]
    force: bool,
    /// Diagnostic origin hint recorded in the receipt (set by curl installers)
    #[arg(long, hide = true, default_value = "native")]
    origin: String,
    /// Accept a publisher signing key this machine has not seen for the package's namespace
    /// (only after checking the publisher really rotated its key)
    #[arg(long)]
    accept_new_key: bool,
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
    /// Reinstall even when the package is already current (pins still hold)
    #[arg(long, conflicts_with_all = ["version", "channel"])]
    force: bool,
    /// Accept a publisher signing key this machine has not seen for a package's namespace
    #[arg(long)]
    accept_new_key: bool,
    #[command(flatten)]
    m: Mutate,
}

#[derive(Args, Clone)]
struct DistUpgradeArgs {
    /// Permit incompatible (major, or 0.x minor) W3BS package updates; pins still hold
    #[arg(long)]
    allow_major: bool,
    /// Accept a publisher signing key this machine has not seen for a package's namespace
    #[arg(long)]
    accept_new_key: bool,
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
enum AbCmd {
    /// List your experiments
    #[command(visible_alias = "list")]
    Ls {
        #[command(flatten)]
        a: Api,
    },
    /// Create a draft experiment. The first --variant is the control.
    New {
        /// Experiment id your server sends (lowercase letters, digits, dashes)
        id: String,
        /// id, id:weight, id={"json":"parameters"} or id:weight={...}; at least two
        #[arg(long = "variant", value_name = "VARIANT", required = true)]
        variants: Vec<String>,
        /// Display name (default: the id)
        #[arg(long)]
        name: Option<String>,
        /// Randomization unit: user, account, session, device or custom
        #[arg(long)]
        unit: Option<String>,
        /// How long it runs once started, in days (default 30)
        #[arg(long)]
        days: Option<u32>,
        /// Participants per variant before the result is worth reading (default 100)
        #[arg(long = "min", value_name = "N")]
        min: Option<u32>,
        /// Who is eligible, in words
        #[arg(long, value_name = "TEXT")]
        eligible: Option<String>,
        /// What a conversion means, e.g. signup-completed (default: conversion)
        #[arg(long, value_name = "GOAL")]
        goal: Option<String>,
        #[command(flatten)]
        a: Api,
    },
    /// Results per variant: assigned, exposed, converted, rate with a 95% interval
    Show {
        id: String,
        #[command(flatten)]
        a: Api,
    },
    /// Start a draft (fixes its window); it then enrolls participants
    Start {
        id: String,
        #[command(flatten)]
        a: Api,
    },
    /// Stop enrolling new participants; existing assignments still answer
    Pause {
        id: String,
        #[command(flatten)]
        a: Api,
    },
    /// Resume a paused experiment
    Resume {
        id: String,
        #[command(flatten)]
        a: Api,
    },
    /// Close for good (cannot be reopened)
    Close {
        id: String,
        #[command(flatten)]
        a: Api,
    },
    /// The variant a participant gets, as your server would ask for it (sticky)
    Assign {
        id: String,
        /// Opaque participant id (an internal id or a hash, never an email)
        participant: String,
        #[command(flatten)]
        a: Api,
    },
    /// Print the OpenABTest manifest, or with --events the event export, as JSON
    Export {
        id: String,
        #[arg(long)]
        events: bool,
        #[command(flatten)]
        a: Api,
    },
    /// Delete an experiment with its assignments and events
    Rm {
        id: String,
        #[command(flatten)]
        a: Api,
    },
}

#[derive(Subcommand, Clone)]
enum PublishCmd {
    /// Your publisher signing keys (SSH keys bound to a domain)
    #[command(subcommand)]
    Key(PublishKeyCmd),
    /// Sign a release statement with your SSH key and submit it to w3bs.org/packages
    Release(Box<ReleaseFlags>),
}

#[derive(Args, Clone)]
struct ReleaseFlags {
    /// Package identity, `<namespace>/<name>` (e.g. pairux.com/pairux)
    #[arg(long, value_name = "NS/NAME")]
    package: Option<String>,
    /// The release's semver version
    #[arg(long = "version", value_name = "SEMVER")]
    release_version: Option<String>,
    /// The install line: curl -fsSL https://…/install.sh | sh
    #[arg(long, value_name = "LINE")]
    install: Option<String>,
    /// Display name (default: the listing's, else the package name)
    #[arg(long)]
    name: Option<String>,
    /// One-line description (default: the current listing's)
    #[arg(long)]
    description: Option<String>,
    #[arg(long, value_name = "URL")]
    homepage: Option<String>,
    #[arg(long, value_name = "URL")]
    repository: Option<String>,
    /// SPDX license expression
    #[arg(long)]
    license: Option<String>,
    /// A command the package installs (repeat for several)
    #[arg(long = "command", value_name = "NAME")]
    commands: Vec<String>,
    /// A downloadable artifact, target=https-url (repeat; each is downloaded and hashed)
    #[arg(long = "artifact", value_name = "TARGET=URL")]
    artifacts: Vec<String>,
    /// SSH key to sign with (default: ~/.ssh/id_ed25519, id_ecdsa, id_ed25519_sk, id_ecdsa_sk)
    #[arg(long, value_name = "PATH")]
    key: Option<PathBuf>,
    /// Read unset fields from a w3bs.toml ([package] id, version, install, description, …)
    #[arg(long, value_name = "W3BS.TOML")]
    from: Option<PathBuf>,
    /// Build, hash and sign, then print the statement and signature without submitting
    #[arg(short = 'n', long)]
    dry_run: bool,
    #[command(flatten)]
    a: Api,
}

#[derive(Subcommand, Clone)]
enum PublishKeyCmd {
    /// Register an SSH key for a publisher namespace (a domain you control), then prove the
    /// domain with a .well-known line or a DNS TXT record
    Add {
        /// The namespace, e.g. pairux.com
        namespace: String,
        /// SSH key (private key, or its .pub to sign through ssh-agent)
        #[arg(long, value_name = "PATH")]
        key: Option<PathBuf>,
        #[command(flatten)]
        a: Api,
    },
    /// List your registered keys and whether their domain proof holds
    #[command(visible_alias = "list")]
    Ls {
        #[command(flatten)]
        a: Api,
    },
    /// Re-check the domain proof of a key (all your keys when none is named)
    Verify {
        /// SHA256:… fingerprint
        fingerprint: Option<String>,
        #[command(flatten)]
        a: Api,
    },
    /// Revoke a key: its releases stay listed but clients refuse to install them
    Revoke {
        /// SHA256:… fingerprint
        fingerprint: String,
        #[command(flatten)]
        a: Api,
    },
}

#[derive(Subcommand, Clone)]
enum FleetCmd {
    /// Add this machine to your fleet with a one-time token from w3bs.org/fleet, and start
    /// its agent as a systemd service
    Join {
        /// The join token shown on w3bs.org/fleet (fj_…)
        token: String,
        /// Name shown on w3bs.org/fleet (default: the hostname)
        #[arg(long)]
        name: Option<String>,
        /// Do not install the systemd service; run `w3bs fleet agent` yourself
        #[arg(long)]
        no_service: bool,
        /// Share this machine's screen on w3bs.org/fleet: view or control (see `w3bs fleet screen`)
        #[arg(long, value_name = "MODE", value_parser = ["view", "control"])]
        screen: Option<String>,
        /// The local VNC server to share (with --screen)
        #[arg(long, value_name = "HOST:PORT", default_value = screen::DEFAULT_VNC, requires = "screen")]
        vnc: String,
        #[command(flatten)]
        a: Api,
    },
    /// Let the fleet owner watch this machine's desktop on w3bs.org/fleet/<id>/screen, through
    /// a local VNC server (see w3bs.org/docs/fleet-screen.md). Outbound only; view only unless
    /// --control. Run without arguments to see the current setting.
    Screen {
        #[command(subcommand)]
        action: Option<ScreenCmd>,
        #[arg(long, hide = true, value_name = "PATH", global = true)]
        config: Option<PathBuf>,
    },
    /// A headless Kubuntu desktop in Docker for AI agents to work on, shared on w3bs.org/fleet
    /// (KDE Plasma, the Agentic Desktop Layer, a VNC socket; no GPU needed)
    Desktop {
        #[command(subcommand)]
        action: DesktopCmd,
    },
    /// Open a machine's screen on w3bs.org/fleet in your browser (uses your `w3bs login`)
    View {
        /// Machine name or id from `w3bs fleet ls`
        machine: String,
        /// Print the address only
        #[arg(long)]
        no_open: bool,
        #[command(flatten)]
        a: Api,
    },
    /// Run the agent in the foreground: report health every 30s and run queued actions
    /// (refresh, update, restart-desktop) when this machine allows actions
    Agent {
        #[arg(long, hide = true, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// This machine's fleet membership and the health it reports
    Status {
        #[arg(long, hide = true, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// List your machines (uses your `w3bs login`)
    #[command(visible_alias = "list")]
    Ls {
        #[command(flatten)]
        a: Api,
    },
    /// Queue an action on a machine: refresh, update or restart-desktop
    Run {
        /// Machine name or id from `w3bs fleet ls`
        machine: String,
        /// refresh | update | restart-desktop
        action: String,
        #[command(flatten)]
        a: Api,
    },
    /// Take this machine out of the fleet and remove its agent service
    Leave {
        #[arg(long, hide = true, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// Remove a machine from your fleet (its agent stops on its next report)
    Rm {
        /// Machine name or id from `w3bs fleet ls`
        machine: String,
        #[command(flatten)]
        a: Api,
    },
}

#[derive(Subcommand, Clone)]
enum DesktopCmd {
    /// Build (first time) and start the desktop, and share its screen with the fleet owner
    Start {
        /// Also let the owner use the mouse and keyboard from w3bs.org/fleet
        #[arg(long)]
        control: bool,
        /// Screen size
        #[arg(long, value_name = "WxH")]
        geometry: Option<String>,
        /// Rebuild the image even if it is current
        #[arg(long)]
        rebuild: bool,
    },
    /// Stop and remove the desktop (its home directory is kept) and stop sharing it
    Stop,
    /// Whether the desktop runs, and what it shares
    Status,
    /// The container's recent log, and where the session logs are
    Logs,
    /// Ask for a VNC password on w3bs.org/fleet (read from stdin; first 8 characters count)
    Password {
        /// Remove the password (the owner-only socket is then the only lock)
        #[arg(long)]
        off: bool,
    },
    /// Register an AI agent on the desktop and print its MCP setup (Claude Code)
    Agent {
        /// Agent name
        name: String,
        /// What it may do: observe (look only), broad (whole desktop)
        #[arg(long, default_value = "broad", value_parser = ["observe", "broad"])]
        preset: String,
    },
}

#[derive(Subcommand, Clone)]
enum ScreenCmd {
    /// Share the screen: view only (input from the page is dropped) unless --control
    On {
        /// Also let the owner use the mouse and keyboard from the page
        #[arg(long)]
        control: bool,
        /// The local VNC server to share
        #[arg(long, value_name = "HOST:PORT", default_value = screen::DEFAULT_VNC)]
        vnc: String,
    },
    /// Stop sharing; open sessions end within one report interval
    Off,
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
    /// Upgrade the whole machine: the system packages (apk, apt, dnf, pacman, zypper or brew),
    /// then every W3BS package and the w3bs CLI
    DistUpgrade(DistUpgradeArgs),
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

    // Your W3BS account.
    /// Sign in to your W3BS account in the browser (OAuth 2.1 with PKCE). Works over SSH:
    /// shows a code to enter at w3bs.org/device
    Login {
        /// Print the sign-in URL instead of opening a browser
        #[arg(long)]
        no_browser: bool,
        /// Sign in with a code entered at w3bs.org/device on any browser (automatic over SSH
        /// or without a display)
        #[arg(long)]
        device: bool,
        #[command(flatten)]
        a: Api,
    },
    /// Sign out on this machine and revoke its session
    Logout {
        #[command(flatten)]
        a: Api,
    },
    /// Show the W3BS account this machine is signed in to
    Whoami {
        #[command(flatten)]
        a: Api,
    },

    // Your fleet.
    /// Your machines on w3bs.org/fleet: join this one, run its agent, list and act on all
    #[command(subcommand)]
    Fleet(FleetCmd),
    /// Your A/B tests on w3bs.org/ab-testing: create, start, read results, export OpenABTest
    #[command(subcommand)]
    Ab(AbCmd),
    /// Sign package releases with your SSH key (W3BS-SIGN-1): register keys, publish releases
    #[command(subcommand)]
    Publish(PublishCmd),

    // The CLI itself.
    /// Check this machine's W3BS state, trust and environment
    Doctor,
    /// Manage the w3bs CLI itself
    #[command(name = "self", subcommand)]
    SelfCmd(SelfCmd),
    /// Any other `w3bs NAME ...` runs `w3bs-NAME ...` from PATH, e.g. `w3bs desktop` from the
    /// agentic-desktop-layer package.
    #[command(external_subcommand)]
    External(Vec<OsString>),
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
    if let Cmd::External(args) = &cli.cmd {
        external(args, &out);
    }
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

/// Run `w3bs-NAME` with the remaining arguments, replacing this process.
fn external(args: &[OsString], out: &Output) -> ! {
    let name = args.first().and_then(|a| a.to_str()).unwrap_or_default();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        eprintln!("w3bs: unknown command {name:?}; see `w3bs --help`");
        std::process::exit(exit::USAGE);
    }
    let bin = format!("w3bs-{name}");
    let mut cmd = std::process::Command::new(&bin);
    if out.json {
        cmd.arg("--json");
    }
    cmd.args(&args[1..]);
    #[cfg(unix)]
    let err = {
        use std::os::unix::process::CommandExt;
        cmd.exec()
    };
    #[cfg(not(unix))]
    let err = match cmd.status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => e,
    };
    if err.kind() == std::io::ErrorKind::NotFound {
        eprintln!("w3bs: unknown command `{name}` (no `{bin}` on PATH); install it with `w3bs install` or see `w3bs --help`");
    } else {
        eprintln!("w3bs: could not run {bin}: {err}");
    }
    std::process::exit(exit::USAGE);
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
        Err(Cmd::Login { no_browser, device, a }) => {
            let progress = |m: &str| {
                if (!out.quiet && !out.json) || out.verbose {
                    eprintln!("w3bs: {m}");
                }
            };
            ("login", account("login", &a, |o| async move { auth::login(&o, !no_browser, device, &progress).await }).await)
        }
        Err(Cmd::Logout { a }) => ("logout", account("logout", &a, |o| async move { auth::logout(&o).await }).await),
        Err(Cmd::Whoami { a }) => ("whoami", account("whoami", &a, |o| async move { auth::whoami(&o).await }).await),
        Err(Cmd::DistUpgrade(DistUpgradeArgs { allow_major, accept_new_key, m })) => {
            let (_, mk) = match engine_factory(&out) {
                Ok(x) => x,
                Err(e) => return ("startup", Err(e)),
            };
            let tty = Tty { out: out.clone() };
            let system = match distupgrade::system(m.yes, m.dry_run, |p| tty.confirm(p), |msg| tty.progress(msg)) {
                Ok(v) => v,
                Err(e) => return ("dist-upgrade", Err(e)),
            };
            // Approving the system step approves the whole upgrade; when it was skipped, the
            // package step asks for itself.
            let m = Mutate { yes: m.yes || system["status"] == "upgraded", dry_run: m.dry_run };
            let r = mk(&m).update(UpdateRequest { packages: vec![], all: true, version: None, channel: None, allow_major, force: false, accept_new_key }).await;
            (
                "dist-upgrade",
                r.map(|mut res| {
                    res.operation = "dist-upgrade".into();
                    res.data["system"] = system;
                    res
                }),
            )
        }
        Err(Cmd::Fleet(f)) => run_fleet(f).await,
        Err(Cmd::Ab(c)) => run_ab(c).await,
        Err(Cmd::Publish(c)) => run_publish(c).await,
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
        Err(Cmd::External(_)) => unreachable!("external commands exec before the runtime starts"),
        Err(_) => unreachable!("every command is dispatched"),
    }
}

async fn run_publish(cmd: PublishCmd) -> (&'static str, w3bs_core::Result<OperationResult>) {
    match cmd {
        PublishCmd::Key(PublishKeyCmd::Add { namespace, key, a }) => {
            ("publish.key.add", account("publish.key.add", &a, |o| async move { publish::key_add(&o, &namespace, key.as_deref()).await }).await)
        }
        PublishCmd::Key(PublishKeyCmd::Ls { a }) => ("publish.key.ls", account("publish.key.ls", &a, |o| async move { publish::key_ls(&o).await }).await),
        PublishCmd::Key(PublishKeyCmd::Verify { fingerprint, a }) => {
            ("publish.key.verify", account("publish.key.verify", &a, |o| async move { publish::key_verify(&o, fingerprint.as_deref()).await }).await)
        }
        PublishCmd::Key(PublishKeyCmd::Revoke { fingerprint, a }) => {
            ("publish.key.revoke", account("publish.key.revoke", &a, |o| async move { publish::key_revoke(&o, &fingerprint).await }).await)
        }
        PublishCmd::Release(f) => {
            let f = *f;
            let args = publish::ReleaseArgs {
                package: f.package,
                version: f.release_version,
                install: f.install,
                name: f.name,
                description: f.description,
                homepage: f.homepage,
                repository: f.repository,
                license: f.license,
                commands: f.commands,
                artifacts: f.artifacts,
                key: f.key,
                from: f.from,
                dry_run: f.dry_run,
            };
            ("publish.release", account("publish.release", &f.a, |o| async move { publish::release(&o, args).await }).await)
        }
    }
}

async fn run_ab(cmd: AbCmd) -> (&'static str, w3bs_core::Result<OperationResult>) {
    match cmd {
        AbCmd::Ls { a } => ("ab.ls", account("ab.ls", &a, |o| async move { ab::ls(&o).await }).await),
        AbCmd::New { id, variants, name, unit, days, min, eligible, goal, a } => {
            let x = ab::NewExperiment { slug: id, name, variants, unit, days, min, eligible, goal };
            ("ab.new", account("ab.new", &a, |o| async move { ab::new(&o, x).await }).await)
        }
        AbCmd::Show { id, a } => ("ab.show", account("ab.show", &a, |o| async move { ab::show(&o, &id).await }).await),
        AbCmd::Start { id, a } | AbCmd::Resume { id, a } => ("ab.state", account("ab.state", &a, |o| async move { ab::set_state(&o, &id, "running").await }).await),
        AbCmd::Pause { id, a } => ("ab.state", account("ab.state", &a, |o| async move { ab::set_state(&o, &id, "paused").await }).await),
        AbCmd::Close { id, a } => ("ab.state", account("ab.state", &a, |o| async move { ab::set_state(&o, &id, "closed").await }).await),
        AbCmd::Assign { id, participant, a } => ("ab.assign", account("ab.assign", &a, |o| async move { ab::assign(&o, &id, &participant).await }).await),
        AbCmd::Export { id, events, a } => ("ab.export", account("ab.export", &a, |o| async move { ab::export(&o, &id, events).await }).await),
        AbCmd::Rm { id, a } => ("ab.rm", account("ab.rm", &a, |o| async move { ab::rm(&o, &id).await }).await),
    }
}

async fn run_fleet(cmd: FleetCmd) -> (&'static str, w3bs_core::Result<OperationResult>) {
    let local = |op: &'static str, r: w3bs_core::Result<Value>| {
        (
            op,
            r.map(|data| {
                let mut res = OperationResult::new(op);
                res.data = data;
                res
            }),
        )
    };
    match cmd {
        FleetCmd::Join { token, name, no_service, screen: mode, vnc, a } => {
            let share = mode.as_deref().and_then(screen::Mode::parse).map(|mode| screen::Screen { mode, vnc });
            if let Some(Err(e)) = share.as_ref().map(|s| screen::check_addr(&s.vnc)) {
                return ("fleet.join", Err(w3bs_core::Error::usage("BAD_VNC_ADDRESS", e)));
            }
            ("fleet.join", account("fleet.join", &a, |o| async move { fleet::join(&o, &token, name.as_deref(), no_service, share).await }).await)
        }
        FleetCmd::Screen { action, config } => {
            let change = match action {
                None => None,
                Some(ScreenCmd::Off) => Some(None),
                Some(ScreenCmd::On { control, vnc }) => Some(Some(screen::Screen { mode: if control { screen::Mode::Control } else { screen::Mode::View }, vnc })),
            };
            local("fleet.screen", fleet::screen_set(config.as_deref(), change))
        }
        FleetCmd::Desktop { action } => match action {
            DesktopCmd::Start { control, geometry, rebuild } => local("fleet.desktop.start", desktop::start(control, geometry.as_deref(), rebuild)),
            DesktopCmd::Stop => local("fleet.desktop.stop", desktop::stop()),
            DesktopCmd::Status => local("fleet.desktop.status", desktop::status()),
            DesktopCmd::Logs => local("fleet.desktop.logs", desktop::logs()),
            DesktopCmd::Password { off } => local("fleet.desktop.password", desktop::password(off)),
            DesktopCmd::Agent { name, preset } => local("fleet.desktop.agent", desktop::agent(&name, &preset)),
        },
        FleetCmd::View { machine, no_open, a } => ("fleet.view", account("fleet.view", &a, |o| async move { fleet::view(&o, &machine, !no_open).await }).await),
        FleetCmd::Agent { config } => local("fleet.agent", fleet::agent(config.as_deref()).await),
        FleetCmd::Status { config } => local("fleet.status", fleet::status(config.as_deref())),
        FleetCmd::Leave { config } => local("fleet.leave", fleet::leave(config.as_deref()).await),
        FleetCmd::Ls { a } => ("fleet.ls", account("fleet.ls", &a, |o| async move { fleet::ls(&o).await }).await),
        FleetCmd::Run { machine, action, a } => ("fleet.run", account("fleet.run", &a, |o| async move { fleet::run_action(&o, &machine, &action).await }).await),
        FleetCmd::Rm { machine, a } => ("fleet.rm", account("fleet.rm", &a, |o| async move { fleet::rm(&o, &machine).await }).await),
    }
}

/// Run an account command against the resolved origin and wrap its data.
async fn account<F, Fut>(op: &str, a: &Api, f: F) -> w3bs_core::Result<OperationResult>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = w3bs_core::Result<Value>>,
{
    let origin = auth::origin(a.api.as_deref())?;
    let data = f(origin).await?;
    let mut res = OperationResult::new(op);
    res.data = data;
    Ok(res)
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
        PkgCmd::Install(InstallArgs { packages, version, channel, repository, manifest, pin, force, origin, accept_new_key, m }) => {
            ("install", mk(&m).install(InstallRequest { packages, manifest, repository, version, channel, pin, force, origin: Some(origin), accept_new_key }).await)
        }
        PkgCmd::List => ("list", mk(&ro).list()),
        PkgCmd::Info { package } => ("info", mk(&ro).info(&package)),
        PkgCmd::Search(SearchArgs { query, repository }) => ("search", mk(&ro).search(&query, repository.as_deref()).await),
        PkgCmd::Outdated => ("outdated", mk(&ro).outdated().await),
        PkgCmd::Update(UpdateArgs { packages, all, version, channel, allow_major, force, accept_new_key, m }) => {
            ("update", mk(&m).update(UpdateRequest { packages, all, version, channel, allow_major, force, accept_new_key }).await)
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
