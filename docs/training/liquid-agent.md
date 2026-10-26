# Teach an agent (W3BS Liquid Agent)

Show W3BS a task a few times. It trains a small temporal policy (a CfC liquid
neural network, always next to a GRU baseline) and can then suggest the next
step, or perform it with your approval. This is the W3BS side of the
[Unified Liquid Agent Pipeline](https://github.com/InfernetProtocol/infernet-protocol/tree/main/docs/liquid-agent)
PRDs: capture, review, datasets, training through Infernet (locally or on a
trusted c0mpute worker), evaluation, activation and the guarded executor.

**Status (2026-10-26):** phase 0 and 1 plus the W3BS parts of phase 2. The agent
acts only on deterministic **mock sites** (a mail client, a Reddit-like forum and
a LinkedIn-like notifications page). It never touches a real site or account.
The seam for the Agentic Desktop Layer exists (`desktopLayerEnvironment` in
`training/liquid/executor.mjs`) and refuses until that work lands. Real
recordings reach Teach through the recorder adapter route
(`POST .../captures/:id/steps`); the desktop recorder does not call it yet.

## Everything starts off

On a fresh install nothing is recorded, used, uploaded or executed. Five
separate permissions, each off by default, each shown with its state, who
changed it and when:

| Permission         | Lets W3BS                                                     |
| ------------------ | ------------------------------------------------------------- |
| `recording`        | record a scoped Teach session (one app or window, one task)   |
| `training_data`    | build datasets from reviewed episodes and train on them       |
| `network_upload`   | send a reviewed, redacted dataset to a trusted c0mpute worker |
| `remote_inference` | ask a model served elsewhere (not used yet: serving is local) |
| `auto_execute`     | let the agent perform actions (external writes still ask you) |

Only a person can turn a permission on (the local app, or a terminal with a
typed code). Anyone, an agent included, can turn one off, and it takes effect
at once: revoking `recording` stops the running recording; revoking
`auto_execute` drops every acting mode back to Suggest and cancels pending
approvals. Every change is in the audit log with timestamp, scope, actor and
whether it was a revocation.

The **kill switch** stops every agent action before the next one runs. Anyone
can throw it; only a person can release it. The executor reads it fresh from the
store immediately before each action, so a switch thrown while you are deciding
on an approval still wins.

## Wireflow

```
Teach page (w3bs training open, tab "Teach")
 |
 |-- Permissions ........ turn on recording / training data / upload / act
 |-- Kill switch ........ stop all agent actions
 |
 1 Record   choose task + ONE app or window, profile (strict | standard),
 |          screenshot hashes (off) -> Start
 |          header shows "● Recording <app> · <task> · N steps" on every page
 |          Pause / Resume / Stop / Discard
 |          (or "Record 20 on the mock sites": a scripted person does the
 |           three tasks inside ordinary Teach recordings)
 v
 2 Review   timeline: screen (app, URL pattern, visible controls), action,
 |          target, text (or "[not kept]") per step
 |          labels: goal, success, failure_reason, risk, reviewed
 |          redact text / target names (undo keeps the original on this device)
 |          Find duplicates; proposed corrections wait here until accepted
 v
 3 Dataset  every reviewed episode, secret scan, split by recording session
 |          -> DatasetBundleV1 (dataset hash, counts, consent, retention)
 |  Train   "Train local" (default): infernet model train ... --family cfc --baseline gru
 |          "Trusted c0mpute worker": upload preview (dataset hash, episode count,
 |          worker peer id, privacy note, cost cap, retention) -> "I understand,
 |          send it" -> c0mpute infernet temporal train ... --yes
 v
 4 Evaluate closed loop on the mock sites (seen layouts + a renamed layout)
 |          task success, autonomous success, interventions, approvals asked,
 |          latency p50/p95, failure modes, per workflow
 |  Activate only after a passing evaluation; shows family, source, weights hash
 v
 5 Use      per task: Off | Suggest | Execute with approval | Bounded automation
            "Run on mock site"; "Waiting for you": Approve / Deny, Do it / Stop
            Audit log of every proposal, approval, denial and executed action
```

## Modes

- **Off** (default for every task).
- **Suggest**: recommended only when the capability check passes (an activated
  model with a passing evaluation, model files present, Infernet installed) and
  you turn it on. The agent never acts; you do the step yourself.
- **Execute with approval**: needs `auto_execute`. You approve every action.
- **Bounded automation**: needs `auto_execute` and explicit bounds (`max_steps`,
  allowed action types). Actions inside the bounds run without asking.

In **every** mode, Bounded included, every external write (submit, send, post,
publish, delete, or a click on a control that does one, such as "Send" or
"Submit post") waits for a person's approval at action time. External writes
cannot be put inside the bounds.

## The guarded executor

A proposal never authorises anything. Before each action the executor re-checks
against the current screen: the proposal validates against
`ActionProposalV1`; its session id and sequence index match the request; its
model hash is the activated model's; the screen did not change since the
prediction (stale observation); the target is visible now; step and time
bounds; the mode and the permission for it (read fresh); and the kill switch
(read fresh). A failed check means a person takes over or the run stops; it is
never retried silently. Duplicate, late or skipped sequence ids from the
runtime (`E_SEQ_DUPLICATE`, `E_SEQ_LATE`, `E_SEQ_GAP`) and unknown or expired
sessions stop the run and are logged. Three refused actions in a row stop it
too. Switching the signed-in account closes the session and opens a new one at
sequence 0 (sessions are scoped to a hash of the account, never its name).

When you deny an action or act differently from a suggestion, that run becomes
a **proposed** correction episode. It joins no dataset and goes nowhere until
you accept it for review and review it.

## Data

- Observations are semantic: app, URL pattern (no query, fragment or
  credentials), focus, visible controls `{role, name}`, a loading flag. Actions
  carry explicit timestamps; the previous action is implicit in the order.
- Masked at source, before anything is stored: password, passcode, PIN, OTP,
  token, secret, API key, card, CVV and similar fields never carry text
  (`text_redacted: true`). The strict profile (default) keeps no typed text at
  all; standard keeps typed text only when the secret scan finds nothing in it.
  Visible names the scan flags become `[redacted]`.
- No credentials, cookies or raw screenshots in episodes. `image_ref` is a
  sha256 or nothing, and only when you chose screenshot hashes for the session.
- The secret scan (the same patterns as Infernet's) runs again before every
  export, upload and training run.
- Episodes are sealed at rest with the Training workspace key. A dataset bundle
  is written in plaintext (mode 0600) under the Training home so the Infernet CLI
  can read it; the copy sent to a c0mpute worker is deleted after the job.

Format-0.1 episodes from the existing Training store convert with
`w3bs training teach episodes import EP` (visible controls are the target and
the focused control, which is all format 0.1 records).

## Threat model

| Threat                                                | Control                                                                                                                                                                                   |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Recording starts without the person knowing           | `recording` off by default; a person starts each session; persistent indicator; revoke stops it at once                                                                                   |
| Recording captures other apps                         | steps from any app but the chosen one are dropped and counted                                                                                                                             |
| Secrets in typed text or on screen                    | masked at capture by field and by scan; scanned again before export, upload, training                                                                                                     |
| A dataset leaves the device silently                  | `network_upload` off; preview with hash, count, worker, cost cap, retention; explicit confirm; `--yes` only after it                                                                      |
| **An untrusted worker reads the data**                | **not prevented**: a worker sees the redacted dataset in plaintext while training. Encryption in transit does not hide it. Local training is the default; choose only a worker you trust. |
| A model posts or sends without consent                | every external write asks a person in every mode; the mock sites also refuse writes without an approval id                                                                                |
| A stale or replayed proposal acts on the wrong screen | re-check against the current observation, sequence index, model hash, visible target                                                                                                      |
| A runaway agent                                       | kill switch read before every action; step and time bounds; stop after three refused actions                                                                                              |
| Another account's state leaks into a run              | session per account hash; account switch resets the session                                                                                                                               |
| A web page drives the local API                       | same human session cookie, loopback Host, `x-w3bs-training` header and Origin checks as the rest of the local app                                                                         |
| An agent grants itself permissions                    | granting, recording, reviewing, exporting, training, activating and approving are human-only; agents may only revoke                                                                      |
| A model that looks good in training is trusted        | activation needs a passing closed-loop evaluation; a liquid family is recommended only when it beats the GRU baseline                                                                     |
| Silent continual learning                             | corrections are proposed episodes; nothing is added to a dataset or uploaded without review                                                                                               |

Known limits: the mock sites are not real sites; evaluation numbers are about
them only. A same-user program can read the Training home (as in the rest of
Training). Retention of the dataset file follows the dataset's `retention.days`
record but is not yet enforced by the store's garbage collector.

## How to run

```sh
w3bs training open                       # tab "Teach"
```

Or from a terminal (granting and recording need a typed code):

```sh
w3bs training teach permissions grant recording
w3bs training teach permissions grant training_data
w3bs training teach demo --count 20      # or: teach record --task email.reply_draft --app mail
w3bs training teach episodes             # then: episodes review EP --success yes
w3bs training teach dataset
w3bs training teach train DS_ID          # needs `infernet` (infernet model runtime setup --yes)
w3bs training teach models
w3bs training teach evaluate MODEL_ID
w3bs training teach activate MODEL_ID
w3bs training teach mode email.reply_draft suggest
w3bs training teach run email.reply_draft
w3bs training teach kill-switch on
```

Remote training: `w3bs training teach train DS_ID --worker PEER_ID --max-price 2
[--public-url URL]` (needs `network_upload`; shows the preview first).

Binaries come from `PATH`, or `W3BS_INFERNET_BIN` / `W3BS_C0MPUTE_BIN` (a
command line such as `node /path/to/infernet/apps/cli/index.js` works).

### Local API

All under `/api/v1/training/liquid` in the local app, behind its human session.
The PRD's routes map as:

| PRD                                  | Local app                                          |
| ------------------------------------ | -------------------------------------------------- |
| `POST /training/episodes`            | `POST /api/v1/training/liquid/episodes`            |
| `POST /training/datasets`            | `POST /api/v1/training/liquid/datasets`            |
| `POST /training/evaluate`            | `POST /api/v1/training/liquid/evaluate`            |
| `POST /training/jobs`                | `POST /api/v1/training/liquid/jobs`                |
| `GET /training/jobs/:id`             | `GET /api/v1/training/liquid/jobs/:id`             |
| `POST /training/models/:id/activate` | `POST /api/v1/training/liquid/models/:id/activate` |

plus `permissions`, `kill-switch`, `captures`, `episodes/:id/review|redact|unredact`,
`jobs/preview`, `models`, `tasks/:task/mode|capability`, `runs`, `asks` and
`audit`. There are no MCP tools for the Liquid Agent: every operation is a
person's decision or a revocation, and the Training MCP server stays read and
propose only.

### Tests

```sh
bun test test/training-liquid.test.mjs     # fake Infernet/c0mpute (test/liquid-fake-infernet.mjs)
INFERNET_LIQUID_PYTHON=/path/to/venv/bin/python \
W3BS_INFERNET_BIN="node /path/to/infernet/apps/cli/index.js" \
  bun test test/training-liquid-real.test.mjs   # real CfC + GRU training and serving
```

The contract (`training/liquid/contract/`) is vendored from Infernet; its
README names the commit and how to update it.
