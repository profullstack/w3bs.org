# W3BS Package Manager

Install from any participating website or with `w3bs install`. Manage every package. Update all with one command.

```sh
curl -fsSL https://w3bs.org/install.sh | sh
```

That downloads the prebuilt `w3bs` binary for your machine, checks its SHA-256, and lets it install itself from the signed W3BS repository. Nothing else is required: no Node, Python, Rust or compiler. Supported: Linux (glibc 2.28+) and macOS 11+, on x86_64 and arm64. WSL2 counts as Linux.

The command is `w3bs`. It lives in `~/.local/bin` (add that to your `PATH` if the installer says so).

## Everyday use

```sh
w3bs search agent                 # search the catalog and every repository you trust
w3bs install claude-code          # install by short name (or by full id: anthropic.com/claude-code)
w3bs list                         # everything W3BS manages, no network
w3bs info claude-code             # where it came from, what it owns, its history
w3bs outdated                     # what has updates, what is pinned or held
w3bs update                       # update everything eligible
w3bs update moshcode              # or just one
w3bs remove claude-code           # remove it (your config and data are kept)
```

Every command takes `--json` for one machine-readable result object on stdout, `--dry-run` (`-n`) to see the plan without changing anything, and `--yes` (`-y`) to approve without a prompt. Prompts read the terminal (`/dev/tty`), never a pipe, so a script cannot answer them for you.

Exit codes: `0` ok (being current, pinned or held is not an error), `1` a package operation failed, `2` bad arguments or an ambiguous name, `3` approval or a trust decision is needed, `4` a global security/state problem, `5` another `w3bs` is running, `130` interrupted.

## Two kinds of package

**Catalog tools** are listed in the [package catalog](/packages). Each one installs with its publisher's own `curl … | sh`; that is the listing rule. `w3bs install` fetches that installer over HTTPS, keeps a copy with its SHA-256, runs it, and records it. `w3bs update` runs it again. The installer may use npm, pip or anything else inside; `w3bs` never calls them itself. W3BS does not own the files a publisher's script writes, so it cannot verify or roll them back, and `w3bs remove` only unregisters the tool and points you at the publisher's uninstall steps.

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

The formats (manifest, package index, channel index, catalog, receipt, repository profile) are the open [OpenPkg](https://logicsrc.com/docs/openpkg) specification. `w3bs` is its reference implementation. Source: [`package-manager/`](https://github.com/profullstack/w3bs.org/tree/main/package-manager).
