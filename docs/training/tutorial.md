# Tutorial: from one demonstration to an agent that uses it

This walks one task through the whole loop on your own machine: import,
review, version, approve, release, connect an agent, retrieve, report, export,
delete. No account, cloud service or paid model is involved.

This tutorial starts from a synthetic bundle so it works without a desktop; the
section after this one shows how to record your own.

## Recording your own task instead

With the Agentic Desktop Layer running and agent access enabled, click the tray
icon and choose **Start training…**. The control app opens at Training: pick
**Teach this task**, say what you are about to do, tick the applications it may
see, and start. Do the task, then tray → **Finish task** → **Complete**. The
recording goes straight into W3BS Training (step 2 below). From a terminal:

```sh
echo 'Write the release notes in Kate' > goal.txt
w3bs desktop training teach --goal-file goal.txt --app org.kde.kate
w3bs desktop training finish --outcome success
```

## 0. Setup

Install the binary (`curl -fsSL https://w3bs.org/training/install.sh | sh`), or
from a checkout of `profullstack/w3bs.org`:

```sh
bun install
alias w3bs-training='bun run --silent training'
export W3BS_TRAINING_PASSPHRASE='choose a long passphrase'   # or let libsecret/Keychain hold the key
bun training/fixtures/write-sample.mjs /tmp/release.w3bs-training.zip
```

With the `w3bs` CLI installed and `w3bs-training` on `PATH`, every command below
also runs as `w3bs training ...`.

## 1. Import

```sh
w3bs-training import /tmp/release.w3bs-training.zip --preview
w3bs-training commit IMPORT_ID
```

The preview shows the capture profile, counts, rights, privacy findings (here:
one text value removed by policy) and alignment. Nothing is searchable until
you commit.

## 2. Review

```sh
w3bs-training review      # terminal; or: w3bs-training open
```

The card says the goal is suggested and the result unknown. Press `c`
(Complete). That confirms the goal and the outcome as yours. In the local app
the same card has Complete / Partial / Failed / Unknown / Rename / Discard.

Human-only steps ask you to type a short code. Without an interactive terminal
they are refused, so an agent with shell access cannot approve for you.

## 3. Curate a version

```sh
w3bs-training datasets create --name "Release workflows"
w3bs-training datasets add DATASET_ID EPISODE_ID
w3bs-training versions create DATASET_ID
w3bs-training datasets inspect DATASET_ID     # readiness per purpose
w3bs-training versions approve DATASET_ID 1 --purpose private_retrieval
w3bs-training versions release DATASET_ID 1
```

Approval names purposes one by one. `private_retrieval` does not imply
`model_training` or `public_distribution`.

## 4. Connect an agent

```sh
w3bs-training setup claude-code --consumer claude-code
#   claude mcp add w3bs-training -- w3bs training mcp serve --stdio --consumer claude-code
echo '{"consumer":"claude-code","dataset_id":"DATASET_ID","version":1,"days":30}' > consumer.json
w3bs-training consumers add --config-file consumer.json
```

Now the agent has these tools: `training_search_datasets`,
`training_retrieve`, `training_report_run`, `training_propose_label`,
`training_request_approval`. It has no tool to approve, release, export,
delete or act on the desktop.

Ask it to do a new task, for example "draft the announcement for the next
release". A well-behaved agent calls `training_retrieve`, re-observes the screen
through the Agentic Desktop Layer (whose broker still authorizes every action),
then reports with `training_report_run`, once with demonstrations and once as
a `baseline`.

## 5. Look at the evidence

```sh
w3bs-training runs
w3bs-training evaluate
w3bs-training inbox       # failed or uncertain runs wait here for you
```

`evaluate` compares success with and without demonstrations, with confidence
intervals. It says plainly when there is not enough evidence for a claim (the
beta bar is 100 held-out tasks across 10 workflow families).

## 6. Export (optional)

```sh
w3bs-training versions approve DATASET_ID 1 --purpose model_training
w3bs-training export DATASET_ID --version 1 --format next-action --out ./export
```

`next-action` writes one example per eligible step: goal + earlier steps + the
frame before the action as input, the action as target. Nothing after the
action leaks in. Failures and unverified steps are listed in `excluded.jsonl`.
The copy lives outside W3BS from then on.

Note: a released version's approvals can still be extended (approval is per
purpose), but its episodes and labels never change.

## 7. Delete

```sh
w3bs-training forget --last 1h
w3bs-training deletions REQUEST_ID
```

The report lists what was removed and every export that cannot be recalled.
