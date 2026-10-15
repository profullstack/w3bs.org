# W3BS Training threat model (M1)

Scope: the local Training store, CLI, MCP server and local app in `training/`.
The desktop recorder has its own model in `desktop/docs/`.

## Assets

1. Captured frames, events and labels: private by default, may contain
   anything that was on screen.
2. Human decisions: confirmations, purpose approvals, releases, grants.
3. Released dataset versions and the grants that expose them to agents.
4. The workspace artifact key.

## Actors

| Actor                                                    | Trusted for                                                           |
| -------------------------------------------------------- | --------------------------------------------------------------------- |
| The person (local user at their own terminal or browser) | everything                                                            |
| An agent connected over MCP                              | reading what it was granted, reporting runs, proposing labels, asking |
| An agent with shell access as the same user              | nothing beyond what any same-user program can do (see limits)         |
| Another web page open in the same browser                | nothing                                                               |
| The author of an imported bundle                         | nothing: every byte is untrusted data                                 |
| A model provider                                         | nothing; no provider is called in M1                                  |

## Threats and controls

| Threat                                                                                 | Control                                                                                                                                                | Test                                                              |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------- |
| Agent confirms its own labels, releases data, grants itself access, exports or deletes | Service-level `requireHuman`; MCP actor is always `agent`; MCP has no such tools                                                                       | `agents cannot perform human-only operations`; MCP tool list test |
| Agent with a shell pipes "yes" into a human command                                    | Human commands need a TTY plus a typed random code; no `--yes`                                                                                         | CLI refusal test (exit 3)                                         |
| Agent reads demonstrations it was not granted                                          | Authorization joins grants, released state and approved purpose **before** full-text search                                                            | `retrieval authorizes first...`; stranger MCP consumer test       |
| Revoked/expired access lingers                                                         | Grants expire; revocation and the agent-access switch are checked on every call                                                                        | `grants expire`; revoke test                                      |
| Prompt injection inside demonstrations                                                 | Returned as `untrusted_text`, under a notice that it is evidence, never instructions; retrieval grants no actions; coordinates omitted                 | injection test                                                    |
| Released version silently changes                                                      | Versions snapshot episode bodies; retrieval and export read the snapshot                                                                               | snapshot test                                                     |
| Hostile archive (traversal, symlink, bomb, executable)                                 | Bounded ZIP reader never touches the filesystem                                                                                                        | zip tests                                                         |
| Script or HTML smuggled as an image                                                    | Magic-byte check; UI renders text with `textContent`; strict CSP; `nosniff`                                                                            | smuggled-HTML import test; CSP test                               |
| Secrets in captured text                                                               | Text values off by default; secret patterns redacted before persistence; blocking warnings require a person                                            | privacy test                                                      |
| Another web page drives the local app (CSRF, DNS rebinding)                            | Loopback bind; Host allowlist (421); one-time 60 s launch link; HttpOnly SameSite=Strict cookie; custom header and Origin check on writes; no CORS     | local app test                                                    |
| Artifact theft from disk                                                               | AES-256-GCM per artifact (AAD = artifact id); key from passphrase (scrypt) or OS key store; no plaintext fallback                                      | sealed-at-rest test                                               |
| Deleted data survives in derived stores                                                | Purge removes labels, warnings, FTS rows, dataset membership, version snapshots, frames; tombstones block re-import; report names exports outside W3BS | forget test                                                       |
| Future-state leakage into training data                                                | Next-action input is built only from the goal, prior steps and the pre-action frame                                                                    | next-action test                                                  |
| Failures taught as successes                                                           | Non-success episodes excluded from imitation exports; failed agent runs go to review                                                                   | export and evaluation tests                                       |

## Known limits (not defended in M1)

- **Same-user malware or an agent controlling a real terminal or the user's
  browser** can do what the user can. TTY confirmation and the launch link
  raise the bar for ordinary agent shells; they are not an OS security
  boundary. The desktop layer's separate native control channel is the
  stronger boundary and will gate recording.
- **Metadata at rest is not encrypted.** The SQLite file (`0600`) holds goals,
  target names and window titles after scrubbing. Artifacts are encrypted.
  Encrypting metadata is planned with the hosted/sync work.
- **Secret detection is pattern-based** and will miss secrets it does not
  recognize. Review is required before any approval for that reason.
- **The launch link appears on the terminal and in the browser's process
  arguments** for up to 60 seconds and works once.
- **Exports and trained models cannot be recalled.** Every export path says so
  and every deletion report lists exports by destination.
