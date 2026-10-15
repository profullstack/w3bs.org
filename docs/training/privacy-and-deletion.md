# Privacy, deletion, cost and recovery

## What is kept, and where

Everything lives in `~/.local/share/w3bs/training` (or `W3BS_TRAINING_HOME`):

| File                   | Contents                      | Protection                                                 |
| ---------------------- | ----------------------------- | ---------------------------------------------------------- |
| `training.sqlite`      | metadata, labels, audit       | mode 0600; not encrypted (see limits)                      |
| `artifacts/*.bin`      | frames and video              | AES-256-GCM, one key per workspace                         |
| `quarantine/<import>/` | an import awaiting commit     | sealed with the same key; removed on commit or after 1 day |
| `key-salt.json`        | scrypt salt (passphrase mode) | not secret                                                 |

The key comes from `W3BS_TRAINING_PASSPHRASE` or the OS key store (libsecret on
Linux, Keychain on macOS). If neither is available, imports fail with
`KEY_UNAVAILABLE`. There is no plaintext fallback.

Encryption protects files at rest. It does not protect a running session from
malware with your privileges.

## Defaults

- Text values: off. Clipboard and audio: not captured by anything in Training.
- Secrets: pattern-redacted before storage, with a blocking warning a person
  must clear. Patterns miss things, which is why every episode needs review
  before approval.
- Sign-in flows, password managers, private browsing: flagged as blocking.
- URLs: query strings and fragments dropped.
- Private review pages load no analytics, trackers or remote assets.

## Retention

| Data                | Default               |
| ------------------- | --------------------- |
| Unreviewed episodes | 7 days                |
| Reviewed episodes   | 90 days unless pinned |
| Audit records       | 30 days               |
| Uncommitted imports | 1 day                 |

Set these in `config.json` (`retention.unreviewed_days`, `approved_days`,
`audit_days`, `quarantine_days`; `review_budget`). Pinning keeps an episode
past retention. It never overrides a deletion.

## Deleting

```sh
w3bs training forget --last 10m          # everything captured in the window
w3bs training episodes delete EP_ID ...  # specific episodes
w3bs training datasets delete DS_ID --dry-run   # then --confirm
```

The local app has the same controls (Settings, and Delete on every episode).
Deletion works whether or not any desktop automation is enabled.

`forget` deletes by **capture time**: every episode overlapping the window,
every frame and event inside it, and uploads not yet committed. It removes
labels, warnings, search index rows, dataset membership and version snapshots
of those episodes. Ids are tombstoned so the same data cannot be re-imported by
accident.

Each deletion returns a report (`w3bs training deletions REQUEST_ID`) that
lists:

- what was removed from the active store;
- dataset versions that listed the deleted episodes;
- **exports that left W3BS**, by destination. These cannot be recalled;
- backups (none are made by the local store) and external providers (none in M1).

## What deletion cannot do

- Recall a copy you exported. Desktop Disable, revoking a version and deleting
  episodes all leave exported files untouched. Every export says so.
- Reverse a model already trained on an export. No machine unlearning is
  claimed.
- Remove copies your own backup tools made of the store directory.

## Cost controls

The cloud inference and training budget is **$0**. Nothing in Training calls a
paid model or provider. `jobs submit` refuses with
`TRAIN_ADAPTER_UNAVAILABLE` until a verified adapter exists, and when one does
it will need an explicit per-job maximum and human approval.

## Disaster recovery

- The store is one directory. Back it up while `w3bs training` is not running
  (SQLite WAL). Restoring it restores exactly what was there, including
  deletions recorded as tombstones.
- Losing the passphrase or the OS key-store entry makes artifacts
  unrecoverable by design. Metadata stays readable.
- A restore from an older backup re-introduces episodes deleted after that
  backup was taken. Re-run the recorded deletions (`deletion_requests` table)
  after restoring; automated tombstone replay on restore comes with the
  hosted backend.
