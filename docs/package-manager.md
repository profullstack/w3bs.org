# Packages in the w3bs CLI

`w3bs` is the W3BS command line: one binary for every W3BS surface. Packages are its first app (install from any participating website or with `w3bs install`, manage every package, update all with one command). Resources are the second: `w3bs resolve`, `w3bs verify` and `w3bs resource …` work with signed `w3bs://` resources in a W3BS registry.

```sh
curl -fsSL https://w3bs.org/install.sh | sh
```

That downloads the prebuilt `w3bs` binary for your machine, checks its SHA-256, and lets it install itself from the signed W3BS repository. Nothing else is required: no Node, Python, Rust or compiler. Supported: Linux (glibc 2.28+) and macOS 11+, on x86_64 and arm64. WSL2 counts as Linux.

The command is `w3bs`. It lives in `~/.local/bin` (add that to your `PATH` if the installer says so).

## Everyday use

```sh
w3bs packages                     # the whole catalog (same list as w3bs.org/packages)
w3bs packages --agentic           # agentic tools only; --ours for Profullstack's; -l for descriptions
w3bs search agent                 # search the catalog and every repository you trust
w3bs install claude-code          # install by short name (or by full id: anthropic.com/claude-code)
w3bs list                         # everything W3BS manages, no network
w3bs info claude-code             # where it came from, what it owns, its history
w3bs outdated                     # what has updates, what is pinned or held
w3bs update                       # update everything eligible
w3bs update moshcode              # or just one
w3bs update --force moshcode      # reinstall even when it is already current (install takes --force too)
w3bs dist-upgrade                 # the whole machine: system packages (apk/apt/dnf/pacman/zypper/brew), then everything above
w3bs remove claude-code           # remove it (your config and data are kept)
```

`install` and `update` check the installed version first and change nothing when it is already the latest; `--force` reinstalls anyway.

The full package manager is `w3bs pkg …` (`w3bs pkg --help`); the common commands above are shortcuts for it.

Every command takes `--json` for one machine-readable result object on stdout, `--dry-run` (`-n`) to see the plan without changing anything, and `--yes` (`-y`) to approve without a prompt. Prompts read the terminal (`/dev/tty`), never a pipe, so a script cannot answer them for you.

Exit codes: `0` ok (being current, pinned or held is not an error), `1` a package operation failed, `2` bad arguments or an ambiguous name, `3` approval or a trust decision is needed, `4` a global security/state problem, `5` another `w3bs` is running, `130` interrupted.

## Two kinds of package

**Catalog tools** are listed in the [package catalog](/packages). Each one installs with its publisher's own `curl … | sh`; that is the listing rule. `w3bs install` fetches that installer over HTTPS, keeps a copy with its SHA-256, runs it, and records it. Before running it, `w3bs` asks the tool itself (`<command> --version`) and compares that with the catalog's `latest_version`: when it is already current, `install` records the tool without running anything (so a tool baked into an OS image is picked up as-is) and `update` reports it as current. When either version is unknown, `update` re-runs the installer. The installer may use npm, pip or anything else inside; `w3bs` never calls them itself. W3BS does not own the files a publisher's script writes, so it cannot verify or roll them back, and `w3bs remove` only unregisters the tool and points you at the publisher's uninstall steps.

**Signed packages** come from a publisher's own W3BS repository: [TUF](https://theupdateframework.io/)-signed metadata over prebuilt archives. These get the full lifecycle:

- every artifact's length and SHA-256 is checked against signed metadata before anything is activated;
- versions are immutable: a publisher who changes the bytes of a published version is caught;
- `w3bs update` stays within your installed major version (`0.x`: minor) unless you pass `--allow-major`;
- `w3bs pin <pkg>` holds a version; `w3bs rollback <pkg>` re-activates the previous version and pins it;
- installs and updates are journaled, so an interrupted run recovers to the old or the new version, never a mix (`w3bs repair`);
- a publisher can revoke a bad release, and W3BS will refuse to activate it.

Rollback restores application files only. It never rolls back configuration, databases or migrations.

## Repositories and trust

```sh
w3bs repo add tools https://tools.example/repository/
w3bs repo list
w3bs repo info tools
w3bs repo update            # refresh and verify metadata
w3bs repo remove tools      # refused while something installed uses it
```

The first time you add a repository you are shown its root fingerprint and asked to trust it. That is first-use trust: it pins the publisher's signing keys for every future update. It is not proof of who runs the repository. Trusting one publisher never trusts another. For unattended machines, pass the expected fingerprint (`--root-fingerprint sha256:…`) or root file (`--root root.json`) with `--yes`; `--yes` alone never accepts an unknown root.

`w3bs install atlas` only looks in repositories you have added. If two of them offer `atlas`, W3BS stops and asks you to name one. It never guesses.

### About `curl | sh`

The very first script runs as you before W3BS can verify anything, and any checksum inside it arrives from the same server. That is the trust you place in a publisher's website, the same as any curl installer. To look first: `curl -fsSL https://w3bs.org/install.sh -o install.sh && less install.sh && sh install.sh`. In CI, prefer download-then-run so a failed download cannot look like success.

## Where things go

```text
~/.local/bin/<command>                    launch links
~/.local/share/w3bs/state.sqlite3         what is installed (source of truth)
~/.local/share/w3bs/packages/<pkg>/       immutable version directories + `current`
~/.local/share/w3bs/receipts/             JSON receipts
~/.local/share/w3bs/trust/<repo>/         pinned roots and anti-rollback state
~/.local/state/w3bs/journal/              interrupted-operation journal
~/.cache/w3bs/                            verified downloads, by SHA-256
```

`XDG_*` variables are honoured. `W3BS_HOME=<dir>` relocates everything. No daemon, no background updates, no telemetry.

## Publishers

There are two ways to be in `w3bs`:

1. **Be in the catalog.** Ship a `curl -fsSL https://<your-site>/install.sh | sh` installer, then open a pull request adding your tool to [`catalog/catalog.json`](https://github.com/profullstack/w3bs.org/blob/main/catalog/catalog.json). No installer, no listing.
2. **Run a signed repository** with `w3bs-publish`. Static hosting is enough.

### Malware scanning

Every catalog package is scanned with [ThreatCrush](https://threatcrush.com). The check on your catalog pull request downloads your installer and, if `repository` is a public GitHub repo, its source at the default branch head, and scans both. Nothing is executed. The check fails on a malware indicator (a decoded payload run as code, the environment serialised into a request, or any CWE-506 finding) or a scan that cannot finish. Other findings are listed but do not block.

After listing, w3bs.org re-scans your package whenever it changes: a new `latest_version`, different installer bytes or a new commit. Until the new scan lands the package shows **Scan pending**. The result is on `w3bs.org/packages/<id>` and in `/api/packages/<id>`, and there is a README badge:

```markdown
[![ThreatCrush scan](https://w3bs.org/api/packages/<id>/badge.svg)](https://w3bs.org/packages/<id>)
```

A false positive is fixed at the source: add `// threatcrush-disable-next-line <rule-id>` above the line, or list the path in a `.threatcrushignore`, and the next scan picks it up. Operators re-scan by hand with `bun scripts/scan-packages.mjs --record <id>` in the app container.

```sh
w3bs-publish init ./myrepo                         # keys/ (keep secret) + signed root; prints your fingerprint
w3bs-publish add ./myrepo --id tools.example/atlas --version 1.2.3 \
  --artifact x86_64-unknown-linux-gnu=dist/atlas-linux-x64.tar.gz \
  --artifact aarch64-apple-darwin=dist/atlas-macos-arm64.tar.gz \
  --command atlas=bin/atlas --description "Atlas CLI" --license MIT
w3bs-publish build ./myrepo                        # index, channels, signed TUF metadata -> myrepo/repo/
# upload myrepo/repo/ to https://tools.example/repository/
w3bs-publish installer --package tools.example/atlas --repo-alias tools \
  --repo-url https://tools.example/repository/ --repo-fingerprint "$(w3bs-publish fingerprint ./myrepo)" \
  --manager-fingerprint <from w3bs.org/repo> --manager-version <ver> --manager-sha256 <platform>=<sha> ... -o install.sh
```

The generated `install.sh` installs (or reuses) `w3bs`, registers your repository, and runs `w3bs install` for your package, so curl users and `w3bs install` users end up with the same verified installation. Re-run `w3bs-publish build` before your metadata expires (timestamp: 30 days by default); a weekly cron is enough.

### Publishing a signed release

A catalog listing can carry your own signature, made with an ordinary SSH key ([W3BS-SIGN-1](W3BS-SIGN-1.md)). The registry then proves the listing came from w3bs.org, and your signature proves the release came from you: w3bs.org can list a release but cannot forge one. Any OpenSSH 8.2+ key of type `ssh-ed25519`, `ecdsa-sha2-nistp256` or their hardware (`sk-`) forms works; RSA and DSA are refused. Sign in first with `w3bs login`.

```sh
# 1. Register a key for your domain (default key: ~/.ssh/id_ed25519, id_ecdsa, id_ed25519_sk,
#    id_ecdsa_sk; --key picks another, and a key that only exists in ssh-agent works too
#    if its .pub file is on disk)
w3bs publish key add pairux.com
#    It prints the proof to add, either of:
#      https://pairux.com/.well-known/w3bs-publisher.txt containing  w3bs-publisher=SHA256:…
#      DNS TXT record  _w3bs.pairux.com  "w3bs-publisher=SHA256:…"
w3bs publish key verify               # re-check the proof once it is up
w3bs publish key ls                   # your keys and their state

# 2. Sign and submit a release
w3bs publish release --package pairux.com/pairux --version 0.17.0 \
  --install "curl -fsSL https://pairux.com/install.sh | sh" \
  --artifact linux-x86_64=https://pairux.com/dl/pairux.AppImage
#    Description, name, homepage, repository, license and commands default to the current
#    listing; set them with --description, --command pairux, … or keep them in a w3bs.toml
#    ([package] id, version, install, description, commands, [[artifacts]] target/url) and
#    pass --from w3bs.toml. --dry-run prints the statement and signature without submitting.

# Lost or leaked key: its releases stay listed but clients refuse to install them
w3bs publish key revoke SHA256:…
```

`w3bs publish release` downloads your installer and every artifact to record their SHA-256, signs the JCS bytes of the statement with `ssh-keygen -Y sign -n w3bs-release@w3bs.org`, checks its own signature, then submits it. Your private key never leaves `ssh-keygen` or ssh-agent. Every command takes `--json`.

On install, `w3bs` verifies the signature, pins your namespace's key on first use, and checks the installer's SHA-256 against the signed one before running it: a changed installer is refused and nothing runs. A release signed by a key this machine has not pinned for your namespace is refused until the user passes `--accept-new-key`, so rotate keys deliberately and say so. Unsigned listings install as before and are shown as `unsigned` by `w3bs install` and `w3bs info`. Anyone can check a release offline:

```sh
echo "pairux.com namespaces=\"w3bs-release@w3bs.org\" ssh-ed25519 AAAA…" > allowed_signers
ssh-keygen -Y verify -f allowed_signers -I pairux.com -n w3bs-release@w3bs.org -s release.sig < release.jcs.json
```

The formats (manifest, package index, channel index, catalog, receipt, repository profile) are the open [OpenPkg](https://logicsrc.com/docs/openpkg) specification. `w3bs` is its reference implementation. Source: [`package-manager/`](https://github.com/profullstack/w3bs.org/tree/main/package-manager).
