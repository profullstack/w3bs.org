# W3BS Training + Agentic Desktop Layer Training Mode

## Product Requirements Document

Version: 1.0
Date: October 15, 2026
Status: Proposed implementation specification. This document does not claim deployment or completed features.
Owner: W3BS / Profullstack, Inc.
Recorder: Agentic Desktop Layer at https://w3bs.org/desktop
Consumer application: W3BS Training at https://w3bs.org/training
Command namespaces: `w3bs desktop training` and `w3bs training`
Implementation repository: Extend `profullstack/w3bs.org`; do not create a competing desktop broker.

## 1. Product definition

Build two connected products:

1. **Desktop Training Mode:** an explicitly enabled extension of the Agentic Desktop Layer that observes authorized desktop activity, captures demonstrations, and proposes task and action labels.
2. **W3BS Training:** a local-first application for importing, reviewing, labeling, organizing, versioning, consuming, and evaluating training data. It must work with imported data even when the desktop recorder is not installed.

The human labels intent, important decisions, and outcomes. The software records observable actions and proposes the remaining labels with traceable evidence.

The complete product loop is:

    Observe or teach -> segment -> review -> approve -> version dataset
      -> retrieve examples or train through a supported adapter
      -> evaluate on new tasks -> review failures -> improve the dataset

Recording is not training. Generating a workflow description is not successful automation. Exporting a file is not completing a model-training job. The UI must distinguish these states.

The first useful release must let an authorized agent consume approved demonstrations and attempt a new task. A video recorder plus an upload dashboard is insufficient.

## 2. Goals, users, and non-goals

### Goals

- Capture a normal authorized workday without constant annotation prompts.
- Produce reusable demonstrations with substantially less effort than labeling individual actions manually.
- Support deliberate, higher-quality "Teach this task" demonstrations.
- Let people inspect, correct, and delete every captured or derived artifact.
- Deliver the same dataset operations through web/PWA, desktop, CLI, TUI, API, and MCP.
- Enable example retrieval immediately and optional model training through explicit adapters later.
- Prove usefulness with held-out task results, not recording hours or dataset size.
- Publish an open, versioned data contract without locking users to a model vendor.

### Primary users

An individual teaches workflows; a reviewer curates demonstrations; an agent developer connects consumers and evaluates results; a workspace administrator manages membership, storage, and release permissions.

Local personal use requires no account. Hosted collaboration requires authentication. Administrators cannot silently activate another person's recorder.

### Non-goals

No covert monitoring, employee productivity scoring, universal keylogger, replacement desktop, OS-permission bypass, automatic publication of personal data, guaranteed learning from every recording, or uncontrolled execution of imported workflows.

Continuous microphone recording, autonomous self-training, a public dataset marketplace, and compulsory GPU infrastructure are outside the initial release.

## 3. Existing stack and integration decisions

### 3.1 Verified baseline

The desktop specification defines a local broker, native adapters, separate human controls, and global revocation. Its source workspace contains Rust broker, schema, Linux, macOS, Windows, and CLI crates. Retain this boundary. [S1][S3]

The W3BS root manifest currently pins Bun 1.4.2 and includes Express, PostgreSQL, MCP, and passkey dependencies. Its README identifies PostgreSQL for production. Do not mistake the general Profullstack blueprint for the existing application's exact implementation. [S4][S5]

### 3.2 Selected implementation

| Area                              | Decision                                                                                     |
| --------------------------------- | -------------------------------------------------------------------------------------------- |
| Desktop capture and authorization | Extend the existing Rust broker and supervised adapters.                                     |
| New Training web application      | Next 16 / React 19, Hono API, Tailwind 4 / shadcn/ui.                                        |
| Existing W3BS host                | Preserve its current routes, account identity, runtime conventions, and production Postgres. |
| Desktop Training interface        | Electron, packaged local UI, bundled training CLI; retain native safety controls.            |
| Shared application logic          | TypeScript ESM packages with runtime-specific adapters.                                      |
| Terminal surfaces                 | Commander CLI and `@profullstack/hqtui`.                                                     |
| Media review                      | Reuse `@profullstack/player` for supported video playback.                                   |
| Optional cloud labeling           | `@anthropic-ai/sdk` behind a provider adapter, disabled until explicitly authorized.         |
| Hosted operation                  | Railway, private object storage, SSE job updates.                                            |
| Local data                        | Embedded SQLite/libSQL metadata plus encrypted artifact files; no cloud database dependency. |

The new interface follows the Profullstack application and desktop patterns; the existing Rust safety core is not rewritten in Electron. [S2]

### 3.3 Repository and runtime rules

Add the Training application inside the existing repository. Route `/training` and `/api/v1/training` to the new module without migrating unrelated pages. Use the existing host as the public path router, or its established gateway if present.

Preserve the root Bun lockfile. Add workspaces through a reviewed change rather than introducing a competing root pnpm lockfile. Ship the Node-compatible training CLI with its desktop runtime; users must not install Node separately. Keep Rust toolchain and Cargo locks independently pinned.

Use existing accounts, magic links, and passkeys. If a separate app process needs account federation, reuse the installed Bridges integration rather than creating another identity database.

Before implementation, record an architecture decision covering routing, session propagation, worker supervision, package layout, and tested runtime versions. Preserve existing licenses: the desktop workspace declares Apache-2.0, while the root project declares MIT. Dataset rights are separate from code licenses. [S3][S4]

## 4. Required end-to-end experiences

### A. Observe my work

The user selects applications, browser profiles/origins, and capture permissions. They enable the desktop layer in recording-only mode, leaving agent execution disabled.

The recorder collects permitted events and visual context. At a natural break or manual review, W3BS Training presents task cards with proposed goals, representative steps, and uncertain outcomes.

The user chooses Complete, Partial, Failed, Unknown, Rename, Split, Merge, or Discard. No question is mandatory merely to keep working.

### B. Teach this task

The user supplies a goal, performs the workflow, optionally adds an explanation, and marks the result. The system builds an episode with explicit boundaries and evidence.

The user reviews sensitive content and grants a specific consumption purpose before dataset admission.

### C. Use what was taught

The user approves a dataset version, authorizes an agent to retrieve examples, and gives it a new task. The agent obtains relevant demonstrations through MCP or the API, checks current state, and uses the existing Desktop Layer for separately authorized actions.

The resulting run is compared with an otherwise equivalent run without demonstrations. New failures enter the review inbox; they do not silently become trusted training examples.

### D. Consume external data

The user imports a supported bundle or maps a generic event file. W3BS Training validates, previews, and classifies its fidelity. The user can curate and consume it without installing a recorder.

## 5. Desktop interaction and states

Add these controls to the existing tray, panel, menu bar, and human control application:

    Training: Not recording
    Observe my work...
    Teach this task...
    Pause recording
    Finish task...
    Add explanation...
    Delete recent activity...
    Open W3BS Training
    Recording scope and privacy...
    Disable entire desktop layer

Display recording status persistently with text and an icon, not color alone. Show active scope, paused/excluded state, and whether inference or transfer is permitted. Closing the main window must not hide the safety control.

Recorder states are `idle`, `arming`, `recording`, `paused`, `blocked`, `finishing`, and `error`. Dataset-processing states are separate.

Start idle after installation, login, restart, crash recovery, lock, logout, or suspend. Resuming requires a fresh human action. OS permission persistence must not silently resume recording.

Provide configurable shortcuts for teach/start, mark boundary, explanation, pause, and the existing emergency stop. Check shortcut conflicts during onboarding.

An illustrative review card:

    Prepare a release-announcement draft
    18 observed steps | 2 uncertain labels | 1 privacy warning
    Goal: suggested, awaiting confirmation
    Result: draft visible; completion unknown
    [Complete] [Partial] [Failed] [Rename] [Review] [Discard]

Use a configurable daily review budget, initially five cards. The budget controls interruptions, not automatic approval. Unreviewed items remain unapproved and expire according to retention settings.

## 6. Permissions and stop semantics

### 6.1 Separate grants

Define independent capabilities for:

- Capturing approved visual surfaces and semantic UI context.
- Recording pointer actions and non-text shortcuts.
- Recording permitted text-entry values.
- Capturing clipboard values or push-to-talk explanations.
- Running local analysis or sending selected content to a named provider.
- Syncing to a workspace, retrieving demonstrations, exporting, publishing, or training a model.
- Executing desktop actions through the existing broker.

Each grant binds a principal, device/workspace, scope, purpose, expiry, and revocation generation. An Observe grant does not authorize persistent recording. Recording does not authorize execution or cloud transfer.

Only the separate human-management channel may enable recording, broaden capture, approve a dataset release, or authorize a paid job. Agent tools may request these actions but cannot approve themselves.

### 6.2 Recording-only operation

The desktop layer may be enabled with only a human-issued recorder grant. Agents receive no new mouse, keyboard, file, browser, email, or calendar permissions.

Default deliberate demonstrations reject new agent input leases. Concurrent human/agent sessions require explicit selection and are labeled mixed-origin.

### 6.3 Global Disable

Global Disable closes the gate before cleanup. It stops new capture, mediated observations, local agent access to unreleased records, automatic processing, sync dispatch, and desktop actions. It cancels managed work and discards late sensitive results.

Historical human review and deletion remain available through the human channel. Turning the layer off must not prevent a person from deleting their data.

Device-bound hosted jobs require short-lived device authorization leases. Revocation stops new dispatch immediately when received; loss of contact expires authority within a proposed maximum of 15 seconds. Display pending remote cancellation honestly.

Explicitly exported or independently released dataset copies have their own lifecycle. The export dialog must explain that Desktop Disable cannot recall those copies or reverse completed training. No background process may turn local data into an independent release without approval.

Provide a separate workspace control to revoke dataset access and cancel training jobs. Do not claim cancellation can undo an external provider's completed work. The master switch governs mediated access, not unrelated tools or credentials an agent holds independently.

## 7. Capture requirements

### 7.1 Capture contract

Capture only available, authorized signals:

| Signal             | Required representation                                                                     |
| ------------------ | ------------------------------------------------------------------------------------------- |
| Pointer operation  | Type, buttons, modifiers, coordinate space, timing, origin evidence.                        |
| Keyboard operation | Shortcut or semantic text edit; distinguish physical keys, logical keys, and IME commits.   |
| UI target          | Application/window, role/name, stable identifiers where exposed, bounds, resolution method. |
| Observation        | Relevant screenshot/crop, accessible state, display layout, capture time, scope.            |
| Transition         | Before/action/after references, ordering uncertainty, missing events, timeout status.       |
| Context            | App/browser versions, locale, keyboard layout, scaling, permission and adapter versions.    |

Preserve meaningful drag paths and hover interactions. Coalesce ordinary pointer movement, repeated scroll bursts, and permitted text edits without deleting meaningful state changes.

Text capture is off by default. A text-edit record may retain only the operation and a placeholder. Never reconstruct an excluded value from another modality.

### 7.2 Timing and evidence

Use monotonic timestamps for ordering plus a wall-clock anchor. Record device/session identity, sequence numbers, frame timestamps, event timestamps, and synchronization uncertainty. Represent nanosecond counters as strings where needed to avoid JavaScript precision loss.

Maintain a bounded, policy-filtered in-memory observation buffer so a genuine pre-action frame can be selected. A frame requested after a click must not be relabeled as pre-action merely because it looks similar.

Associate steps with before and after states only when timing supports that association. Otherwise mark the relationship uncertain and exclude it from training modes requiring exact transitions.

Preserve raw permitted event records separately from normalized steps. Deduplicate correlated browser, accessibility, and input reports without pretending they are three actions.

### 7.3 Visual capture

Default to event-driven keyframes and relevant crops, not full-day high-frame-rate video. Allow optional short video segments for workflows needing motion evidence under a separate storage budget.

Apply scope checks and exclusion filtering before persistence, inference, or transfer. Temporary acquisition buffers are sensitive and must be bounded, inaccessible to agents, and excluded from logs/crash dumps where supported.

Application allowlisting alone is not sufficient for whole-screen capture. Unapproved windows, overlays, notifications, and browser tabs may still be visible. Use validated surface capture or masking; pause when strict scope cannot be maintained.

### 7.4 Missing capability

Gaps are first-class records. Capture denial, protected surfaces, inaccessible controls, dropped frames, missing origin evidence, and unsupported IME behavior must remain visible.

Never generate fictional clicks to make a recording look complete. Video-derived action suggestions remain inferred until reviewed and must not be exported as device-observed actions.

## 8. Platform delivery and adapters

KDE Plasma and GNOME are the first certification targets. Windows and macOS are required next. Other Linux desktops follow through capability-based adapters.

| Platform               | Capture design                                                                                                         |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| KDE / GNOME Wayland    | Permissioned ScreenCast/PipeWire, AT-SPI, browser instrumentation, and individually validated compositor integrations. |
| Windows                | Native capture, UI Automation, and permission-respecting input observation where supported.                            |
| macOS                  | OS-approved screen capture, Accessibility, and permitted event observation.                                            |
| X11 and other desktops | Separate adapters and tests; never inherit a Wayland certification automatically.                                      |

The Wayland InputCapture portal redirects input during active capture; it is not a generic passive desktop event tap. Do not steal and reinject the user's entire input stream to simulate observation. Passive capture feasibility must be proven per compositor. [S6]

ScreenCast supplies a capture route, not proof of input-event access. Accessibility reports can reflect programmatic activity and are not proof of human action. [S7][S8][S11]

Advertise capabilities independently:

    visual_capture
    semantic_state
    passive_pointer_events
    shortcut_events
    semantic_text_edits
    human_agent_attribution
    protected_surface_detection
    trustworthy_pre_action_frames
    scoped_surface_capture

Certify data profiles, not just operating systems: `visual-review`, `semantic-demonstration`, and `action-supervision`. A limited profile may be useful, but cannot be marketed as complete action capture.

Use a browser extension with explicit origin permissions and authenticated native messaging when appropriate. Browser instrumentation supplements desktop support; it does not replace it. [S10]

## 9. Labeling and task segmentation

### 9.1 Label hierarchy

Every episode supports goal, boundaries, prerequisites, parameter values/placeholders, outcome, and optional user explanation.

Every step supports observed action, target, before/after states, action arguments, actor origin, and relationship to the task.

Each label independently records value, source, evidence references, author/model version, proposed/confirmed/rejected status, and revision history. A model confidence score is not a guarantee of correctness.

Label sources include `human`, `device_event`, `application_event`, `deterministic_rule`, `model_inference`, and `imported`. Actor origin is separately `human`, `agent`, `system`, `mixed`, or `unknown`.

### 9.2 Automatic labeling pipeline

First normalize observable events deterministically. Then propose targets and transitions from structured metadata. Finally use an explicitly permitted model only for ambiguous labels, summaries, and segmentation.

Use cheap local rules before inference. Cache by sanitized input and processor version. Do not send every screen frame to a model.

Record voluntary explanations as user-authored annotations. Never invent the user's reasoning or present generated rationale as something the person thought.

### 9.3 Segmentation

Explicit teach/start/finish markers take priority. Otherwise combine application context, resource continuity, UI changes, idle periods, repetition, and possible completion signals.

Application switching is not a task boundary by itself. Tasks may contain noncontiguous segments when work is interrupted. Preserve chronology and parent/child task relationships.

The reviewer can split, merge, trim, reorder an explicitly derived instructional view, or exclude steps. Original permitted observations remain unchanged until retention or deletion removes them.

### 9.4 Review efficiency

Prioritize privacy warnings, unresolved goals/outcomes, ambiguous targets, novel workflows, and repeated model disagreements. Sample some high-confidence examples to detect systematic errors.

Group similar tasks and allow bulk acceptance of a narrowly defined label proposal. Do not bulk-confirm completion, ownership, publication rights, or privacy clearance merely because task titles match.

Support keyboard-first review, task-level undo, saved label templates, and batch renaming. Mobile review supports cards and basic corrections; dense timeline editing may use desktop.

### 9.5 Outcomes and mistakes

Outcome values are `success`, `partial`, `failure`, `interrupted`, and `unknown`, with evidence type and verifier recorded separately.

A click on Save is not proof that saving succeeded. A confirmation message supports a UI-level result, not necessarily an external business result.

Preserve mistakes and recovery branches. A clean instructional sequence is a derived artifact; removing the wrong action must not imply that later recovery happened from a different state.

## 10. W3BS Training application

### 10.1 Routes and navigation

`/training` is the public product and documentation entry point. `/training/app` is the authenticated workspace application. Local desktop users get the same review experience against local storage.

Required workspace areas:

| Area      | Function                                                            |
| --------- | ------------------------------------------------------------------- |
| Overview  | Capture status, pending reviews, storage, dataset readiness, costs. |
| Inbox     | Proposed tasks and privacy/quality exceptions.                      |
| Sessions  | Timeline, keyframes/video, event and label inspection.              |
| Datasets  | Collections, membership, versions, splits, rights, releases.        |
| Workflows | Parameterized procedures derived from approved episodes.            |
| Consumers | Agent connections, retrieval grants, training adapters.             |
| Runs      | Retrieval, execution, export, training, and evaluation results.     |
| Settings  | Policies, membership, retention, providers, budgets, deletion.      |

### 10.2 Timeline workspace

Reuse W3BS branding and design tokens. Support a 360-pixel-wide review layout, keyboard navigation, screen readers, visible focus, and non-color status cues. Large timelines must be virtualized without hiding accessible alternatives.

Provide synchronized playback, action markers, target overlays, before/after comparison, gap indicators, and a searchable event list. Show whether evidence is observed or inferred at the point of use.

Selecting a label reveals its evidence, source, revision history, and affected exports. Reviewers can edit without writing JSON.

Redaction tools must operate across image regions, structured text, labels, summaries, and derived previews. A visual blur alone does not remove the corresponding accessible text.

### 10.3 Dataset workspace

Allow creating collections, adding/removing episodes, filtering by task/app/profile/outcome, checking missing labels, and comparing versions.

Show counts of admissible examples per consumer profile. "Ready for human review" is different from "Ready for action-model training."

Every release displays intended use, creator, contributors, source rights, sensitive categories, capture limitations, split strategy, and evaluation coverage.

### 10.4 Consumer workspace

The user chooses an approved version and one of: Retrieve examples, Create a workflow, Export a dataset, Evaluate a consumer, or Train through a configured adapter.

Before submission show exact artifacts, destination, purpose, retained copies, required permissions, estimated cost, and applicable limits.

The Train action is disabled with a clear reason when no verified training adapter supports the dataset modality and target model.

## 11. Import and ingestion

### Required import formats

The initial release supports the native W3BS demonstration bundle, JSON/JSONL through a mapping wizard, and video with optional timestamped annotations.

Later adapters may support additional recorder and benchmark formats after contract tests. Do not claim compatibility merely because a file extension matches.

The mapping wizard previews sample records, validates fields, maps timestamps/actions/targets, and reports what cannot be inferred. Video-only data enters the visual-review profile until sufficient supervision is added.

### Ingestion pipeline

    Receive -> quarantine -> validate manifest and limits -> inspect content
      -> apply privacy policy -> normalize -> preview -> commit import

Use resumable uploads, per-part checksums, idempotent completion, bounded archive extraction, and transactional metadata publication. Partial imports are not searchable or consumable.

Reject path traversal, archive bombs, executable attachments, remote asset auto-fetching, and unsupported schema versions. Inspect media in constrained workers.

Imported HTML, screenshots, instructions, and transcripts are untrusted data. Never execute scripts, shell commands, browser actions, or embedded model instructions during import or playback.

Require the uploader to declare permitted uses and source rights. Missing rights metadata blocks publication and external training, not private quarantine inspection.

## 12. Data contract and portable bundle

Propose a community draft named **W3BS Demonstration Dataset Format**, version 0.1. This is not an existing ratified W3BS standard.

### Core records

- `session`: device context, capture grant, clocks, scope, adapters, capture profile.
- `episode`: task goal, ordered segment references, parameters, outcome, rights, review state.
- `observation`: timestamped visual/semantic context and visibility restrictions.
- `action`: type, arguments, target, coordinate frame, actor origin, provenance.
- `label_revision`: field path, value, evidence, status, processor/reviewer identity.
- `artifact`: content reference, checksum, media type, encryption and retention metadata.
- `dataset_version`: immutable membership, transformations, split manifest, purpose, digest.
- `consumer_run`: exact inputs, consumer version, permissions, cost, output, evaluation.

All records carry workspace ownership, schema version, creation time, lineage, and deletion/revocation status where applicable.

An illustrative step record:

```json
{
  "schema_version": "0.1",
  "episode_id": "ep_demo_001",
  "step_id": "step_018",
  "observation_before": "obs_017",
  "action": {
    "type": "click",
    "origin": "human",
    "source": "device_event",
    "target": {
      "role": "button",
      "name": "Save draft",
      "source": "browser_metadata"
    },
    "monotonic_ns": "18025000000"
  },
  "observation_after": "obs_018",
  "alignment": {
    "status": "verified",
    "evidence": ["event_144", "frame_017", "frame_018"]
  },
  "outcome": {
    "value": "unknown",
    "source": "deterministic_rule",
    "status": "proposed"
  },
  "privacy": {
    "policy_id": "policy_work_apps",
    "text_values_included": false
  }
}
```

The JSON is illustrative, not the complete schema. Implementation must ship JSON Schema definitions, migration fixtures, Rust/TypeScript validators, and valid/invalid examples.

### Bundle structure

```text
manifest.json
sessions.jsonl
episodes.jsonl
observations.jsonl
actions.jsonl
labels.jsonl
artifacts/
splits.json
rights.json
DATASET.md
checksums.json
```

Bundles contain no credentials, executable loaders, absolute private filesystem paths, or silently dereferenced remote URLs.

Use stable resource identifiers and content digests. Apply hashing after required redaction. Do not expose globally searchable fingerprints of private content.

Proposed future resource classes may include `w3bs://dataset/...` and `w3bs://demonstration/...`. Define and test their resolver profiles before advertising working links. Signatures establish integrity and publisher identity, not label accuracy or safe execution.

## 13. Dataset lifecycle and admission

Lifecycle:

    captured/imported -> quarantined -> processed -> review_required
      -> approved_for_purpose -> released_version -> revoked/deleted

Admission requires valid scope/rights, privacy review appropriate to the purpose, sufficient labels for the target profile, and no blocking integrity errors.

Approval must name allowed purposes independently: human review, private retrieval, external retrieval, model training, evaluation, and public distribution. Approval for one does not imply another.

Released dataset versions are immutable manifests. Corrections create a new version; revocation removes future access to an old version. "Immutable" does not mean artifacts must be retained forever or remain accessible after deletion.

Deduplicate and partition related episodes before assigning train/validation/test splits. Keep frames, near-duplicates, repeated task instances, and linked source documents from leaking across incompatible evaluation partitions.

Maintain two evaluation tracks: new instances of known workflows and genuinely held-out workflow families. Do not advertise the first as proof of generalization to the second.

Failures may be admitted as recovery or negative examples with suitable labels. Never mix them into successful imitation examples without an explicit transformation.

## 14. Data consumption modes

### 14.1 Retrieval-assisted agents: initial release

Implement authorized search over approved demonstrations using structured filters and full-text retrieval first. Optional embeddings must obey the same scope, provider, and deletion policies.

An agent supplies its task, available applications, capabilities, and context budget. Return a bounded evidence bundle with goal, prerequisites, selected steps, relevant observations, outcome evidence, limitations, and source version.

Authorize before searching or ranking. Never retrieve a private example and filter it only after generation.

Examples are evidence, not higher-priority instructions. Embedded requests to expose secrets or change permissions have no authority.

Retrieval does not grant execution. Every desktop action still passes through the existing broker's policy and input lease. Re-observe current state; do not blindly replay old coordinates.

Ship tested connection instructions for Claude Code, OpenCode, and Codex at pinned client versions. Their ability to use MCP does not imply their underlying models can be fine-tuned.

### 14.2 Reusable workflows

Derive a reviewable procedure containing purpose, parameters, prerequisites, semantic steps, expected results, stop conditions, human approvals, and source demonstrations.

Replace instance-specific values with typed parameters only after review. Preserve parameter consistency across actions and observations. Keep credentials as runtime references outside datasets.

Do not automatically translate UI observations into service API calls unless an adapter establishes equivalent semantics and the user authorizes the new action scope.

A workflow release is not proof it executes reliably. Mark it untested until a separate evaluation run passes.

### 14.3 Training exports

Provide canonical JSONL plus documented consumer-specific transformations. Include transformation version, excluded records, missing modalities, tokenizer/image assumptions where relevant, and checksums.

For next-action supervision:

    Input = goal + permitted prior history + state available BEFORE the action
    Target = demonstrated action

Keep post-action observations, later labels, completion summaries, and future events out of that input. They may supervise a separate outcome model.

Where a redaction removes task-essential information, exclude or explicitly parameterize the example. Do not preserve correctness by quietly restoring private values.

### 14.4 Actual model-training jobs

Define a provider-neutral adapter with `capabilities`, `validate`, `estimate`, `submit`, `status`, `cancel`, `artifacts`, and `delete_remote_data` where supported.

Adapters declare supported modalities, model identifiers, maximum sizes, training methods, cost controls, retention behavior, and deletion/cancellation limitations.

A job requires an approved version, supported target, configured credentials, execution budget, holdout split, evaluation plan, and human approval. Record the provider job ID and actual lifecycle.

Support external provider APIs or explicitly configured external runners. Keep orchestration in the existing application stack; do not introduce a second Python application server or compulsory local GPU environment.

Unknown external submission results require reconciliation before retry. Never submit another billable job just because a client timed out.

Training completion produces a versioned model/adapter artifact and metrics. Deployment remains a separate approval with rollback to a previously evaluated version.

### 14.5 Dataset exchange

Provide an optional NicheDB adapter after validating its available contracts. Discovery and publication require explicit workspace permission; only approved metadata and artifacts may leave the workspace. The stack's data-sharing conventions must never cause automatic publication of personal recordings.

### 14.6 Feedback loop

Log which examples a consumer used and the resulting outcome. Store those runs separately from demonstrations. Route uncertain or failed runs for review; do not reinforce an agent's mistakes through automatic self-approval.

## 15. Architecture and implementation boundaries

```text
Native human control
  -> existing Rust authorization broker
      -> supervised platform/browser capture adapters
      -> local privacy filter and encrypted capture store
      -> normalized events and candidate episodes

W3BS Training local UI / Electron / CLI / TUI
  -> shared training domain services
      -> local review, datasets, retrieval, export
      -> explicitly authorized outbound sync

W3BS hosted Training app
  -> authenticated Hono API
      -> existing Postgres identity and training metadata
      -> private artifact storage
      -> durable jobs and isolated processing workers
      -> approved consumer/provider adapters

Authorized agent
  -> training MCP retrieval -> approved evidence bundle
  -> separate Desktop MCP -> broker-authorized actions
```

Keep policy enforcement out of presentation code. Native adapters cannot approve grants. Hosted routes cannot directly enable desktop capture.

Use local IPC and narrowly scoped tokens between native components. Preserve the existing broker's browser-request restrictions. Do not enable permissive CORS so the hosted app can call a loopback control endpoint.

The hosted application sees local data only through user-approved outbound sync or import. A phone can review synced data; it cannot silently inspect the user's computer.

Suggested additive layout, adjusted to existing conventions:

```text
apps/training-web/
apps/training-desktop/
packages/training-schema/
packages/training-core/
packages/training-client/
packages/training-cli/
packages/training-mcp/
services/training-worker/
desktop/crates/  # extend existing broker/adapters; add recorder components
docs/training/
```

Use shared service contracts with separate local and hosted storage implementations. Do not fork labeling behavior between surfaces.

## 16. API, CLI, and MCP contract

All names below are proposed interfaces, not claims that commands already exist.

### HTTP

Use `/api/v1/training` with authenticated workspace context, cursor pagination, bounded responses, idempotency keys for job creation, and revision preconditions for edits.

Core resource groups:

```text
GET/POST       /imports
GET            /imports/{id}
POST           /imports/{id}/complete
GET            /sessions
GET            /sessions/{id}
GET            /episodes/{id}
PATCH          /episodes/{id}
POST           /episodes/{id}/split
POST           /episodes/merge
POST           /episodes/{id}/review
GET/POST       /datasets
POST           /datasets/{id}/versions
GET            /datasets/{id}/versions/{version}
POST           /datasets/{id}/versions/{version}/release
POST           /datasets/{id}/versions/{version}/revoke
POST           /retrieve
GET/POST       /exports
GET/POST       /training-jobs
GET/POST       /evaluations
POST           /jobs/{id}/cancel
POST           /deletion-requests
GET            /deletion-requests/{id}
GET            /events
```

Support collection update/delete and episode removal through explicit, audited operations. Artifact reads require current authorization; public URLs are never a substitute.

### CLI

```sh
# Existing Desktop namespace, proposed recording extensions.
w3bs desktop training status --json
w3bs desktop training configure
w3bs desktop training observe --scope work
w3bs desktop training teach --goal-file goal.txt
w3bs desktop training pause
w3bs desktop training finish --outcome success
w3bs desktop training forget --last 10m

# Companion app and dataset consumption.
w3bs training open
w3bs training import ./demonstrations.w3bs-training.zip
w3bs training review
w3bs training datasets create --name "Release workflows"
w3bs training datasets list --json
w3bs training datasets inspect DATASET_ID --json
w3bs training datasets update DATASET_ID --metadata-file metadata.json
w3bs training datasets delete DATASET_ID --dry-run
w3bs training datasets delete DATASET_ID --confirm
w3bs training versions create DATASET_ID --manifest-file selection.json
w3bs training retrieve --dataset DATASET_ID --query-file task.txt --json
w3bs training export DATASET_ID --version VERSION --format jsonl --out ./export
w3bs training consumers add --config-file consumer.json
w3bs training evaluate --plan-file evaluation.json
w3bs training jobs submit --plan-file approved-training-plan.json
w3bs training jobs cancel JOB_ID
w3bs training tui
w3bs training mcp serve --stdio
```

Sensitive text uses files/stdin, not command-line arguments. Human-only operations require human authorization; shell access alone does not turn an agent request into human consent.

Register the companion package as `w3bs-training` through W3BS Package Manager when released. Preserve `w3bs install <package>` and shared `w3bs update` behavior.

### MCP

Expose tools for capability discovery, dataset search, demonstration retrieval, run feedback, job inspection, and approval requests. Use the same domain services as CLI/API.

Agents may create proposed labels but cannot mark them human-confirmed. They cannot enable recording, broaden scope, approve publication, or authorize their own paid training jobs.

Use structured errors such as `SCOPE_DENIED`, `HUMAN_APPROVAL_REQUIRED`, `UNSUPPORTED_CAPTURE_PROFILE`, `DATASET_NOT_RELEASED`, `BUDGET_EXCEEDED`, `REVISION_CONFLICT`, and `REMOTE_OUTCOME_UNKNOWN`.

## 17. Privacy, security, and trust

### Capture privacy

Credentials, authentication flows, password managers, private browser contexts, and explicitly excluded applications are blocked by default. Clipboard and audio are off. Push-to-talk explanations are optional.

A permitted browser origin is not permission for every tab, redirect, frame, or download. Revalidate scope on navigation and focus changes.

Pause capture in strict mode when exclusion cannot be established. Secret detection is defense in depth, not a promise that every secret can be recognized. Explain this residual risk during onboarding.

### Storage and processing

Encrypt local artifacts and sensitive metadata with per-workspace keys protected through available OS key storage. When suitable key protection is unavailable, offer an explicit passphrase-locked mode or block sensitive capture; never silently fall back to plaintext.

Use authenticated encryption through reviewed libraries. Keep decrypted data out of telemetry and provider error reports. Do not claim encryption protects a running session from malware with equivalent privileges.

Hosted server-side processing and true end-to-end encrypted storage are distinct modes. Encrypted-archive mode permits storage/sync without server indexing or training; hosted processing requires explicit permission to decrypt the selected content.

### Application security

Electron renderers require sandboxing, context isolation, disabled Node integration, restrictive CSP, and validated IPC senders. Imported documents are rendered inertly; remote pages receive no native control bridge. [S9]

Use workspace-scoped authorization on every data, search, artifact, job, and export route. Check grants again at job dispatch and output delivery. Test cross-workspace identifiers and revocation races.

Private review pages must not load advertising, session-replay, third-party analytics, or tracking pixels. Public marketing analytics must never receive task names, screenshots, personal identifiers, or private route parameters.

No anonymous training-data reads. Payments or crawler passes never override permissions or missing data rights.

## 18. Storage, retention, sync, and deletion

Use local encrypted artifact files with an embedded metadata store. Hosted artifacts remain in private buckets/volumes, while Postgres stores ownership, indexes, manifests, and job state.

Proposed defaults:

| Data                                 | Default retention                           |
| ------------------------------------ | ------------------------------------------- |
| Temporary unpersisted buffers        | Only the bounded capture/processing window. |
| Unreviewed recordings and candidates | 7 days.                                     |
| Approved private demonstrations      | 90 days, renewable or explicitly pinned.    |
| Metadata-only audit records          | 30 days, configurable.                      |
| Hosted backup expiry after deletion  | Maximum 30 days, disclosed.                 |

Show expiry before recording starts. Pinning cannot override a later deletion request or rights revocation.

Sync is off by default. Use resumable chunk transfers, encrypted transport, checksum verification, conflict-aware label updates, and idempotent commits. Scope changes invalidate queued transfers before dispatch.

Delete recent activity by capture-time interval, not just the currently visible task. Remove matching frames, events, labels, summaries, thumbnails, indexes, embeddings, and uncommitted uploads.

Deletion requests traverse lineage and report active stores, backups, exports, and external providers separately. Block new access immediately; target active-store removal within 24 hours. Reapply deletion tombstones during backup restoration.

Copies exported outside W3BS and already-trained model weights cannot be promised erased. Report external deletion requests and provider limitations without claiming successful machine unlearning.

## 19. Cost, performance, and reliability

### Cost controls

Default cloud inference and training budget: $0. Local deterministic capture and review must function without a paid model.

Require explicit provider configuration, selected artifacts, a per-job maximum, and daily/monthly ceilings. Separate labeling, embeddings, training, and evaluation budgets.

Reserve estimated cost atomically before dispatch. Limit tokens, frames, retries, concurrency, and recursive job creation. Stop launching work when the remaining budget cannot cover the declared maximum.

Reconcile reported usage and distinguish estimates from invoices. External billing latency can limit exact enforcement; do not promise a provider-side hard cap unless the provider supports it.

No silent cloud fallback when a local model fails. No automatic credit purchases or model training.

### Initial engineering targets

These are acceptance targets, not measured results. Benchmark capture separately from optional inference.

| Measure                        | Target                                                                                           |
| ------------------------------ | ------------------------------------------------------------------------------------------------ |
| Recorder overhead              | Under 5% average total CPU and 750 MiB resident memory on a declared 4-core/16 GiB test machine. |
| Default capture volume         | At most 2 GiB per 8-hour reference workload; configurable 5 GiB daily hard cap.                  |
| Local stop feedback            | Within 100 ms; close managed capture streams within 1 second at p95.                             |
| Hosted device-bound revocation | No new authorized dispatch beyond the 15-second lease bound.                                     |
| Retrieval                      | Under 2 seconds p95 for 10,000 indexed episodes on declared reference hardware.                  |
| Review effort                  | Pilot median below 30 seconds for confirming an uncomplicated proposed task card.                |

Use an eight-hour soak test with the actual reference workload and publish hardware, display resolution, event volume, and inference settings.

When storage, battery, or thermal limits are reached, warn and pause or reduce optional capture explicitly. Never silently discard approved examples or label missing data as complete.

Use supervised workers, bounded queues, durable checkpoints, and recoverable partial imports. A restarted recorder remains off. A restarted worker must not duplicate billable submissions.

## 20. Evaluation and acceptance tests

### Dataset quality

Maintain a manually adjudicated fixture set covering applications, languages, layouts, interruptions, failures, and mixed-origin events.

Measure action classification, target accuracy, timing alignment, segmentation quality, goal/outcome agreement, privacy misses, and reviewer correction effort separately.

Initial action-supervision admission target: at least 95% correctly grounded admitted actions on the declared validation set. Do not hide excluded examples; publish eligible coverage and abstention rates alongside accuracy.

Privacy and scope test suites must show zero detected prohibited artifacts reaching storage, retrieval, or providers. Passing those suites is required but is not a universal no-leak guarantee.

### Consumer usefulness

For beta, evaluate at least 100 held-out task instances across at least 10 workflow families. Use synthetic or specifically authorized evaluation data.

Compare identical consumer/model versions and task conditions with versus without demonstrations. Report success, human interventions, unsafe-action attempts, elapsed time, and cost, with uncertainty intervals.

Target a 10-percentage-point success improvement on the chosen suite. Treat it as a product hypothesis. If improvement is unproven, ship an accurately described data tool rather than claiming trained-agent effectiveness.

### Required negative and recovery tests

1. Disable during capture, inference, export, retrieval, and action execution; reject late results.
2. Lock, suspend, crash, adapter hang, permission revocation, or hidden safety control; fail closed.
3. Password fields, unexpected overlays, excluded tabs, redirects, clipboard secrets, and secret-bearing metadata.
4. Multiple monitors, mixed scaling, hotplug, negative origins, rapid focus changes, drag/drop, and IME input.
5. Out-of-order events, duplicate events, stale frames, missing pre-action state, and unknown actor origin.
6. Corrupt archives, path traversal, malicious HTML, prompt-injection text, oversized imports, and schema mismatch.
7. Cross-workspace reads, stale tokens, malicious local browser requests, and self-approval attempts.
8. Concurrent label edits, revoked dataset versions, deleted artifacts, restoration from backup, and expired grants.
9. Network interruption, worker restart, ambiguous provider submission, cost exhaustion, and cancellation failure.
10. Train/test contamination and accidental inclusion of future-state information in model inputs.

## 21. Delivery milestones

Milestones are evidence gates, not calendar promises.

### M0: Integration and capture feasibility

Record source/runtime versions, approve the integration architecture, publish the capability matrix, and demonstrate permissioned capture on KDE and GNOME.

Prove the stop path independently of the UI process. Establish whether each platform supports visual review, semantic demonstration, or complete action supervision.

Deliver the schema draft, threat model, fixtures, and explicit limitations. Existing action-control support is not evidence that passive capture works.

### M1: Local vertical slice

Deliver Teach mode, scoped observation, local storage, basic segmentation, task-card review, native bundle import/export, and the local Training UI.

Demonstrate three useful workflows spanning browser and native applications. Record supported-profile gaps honestly. No cloud account or paid API is required.

Exit gate: a person records a task, corrects labels, approves a dataset version, and deletes it with verified lineage cleanup.

### M2: Real consumption and hosted application

Deliver `w3bs.org/training`, shared authentication, opt-in sync, dataset versions, CLI/TUI/API/MCP parity for core operations, retrieval-assisted consumption, and evaluation runs.

Exit gate: an external agent retrieves an approved demonstration, attempts a new authorized task, and reports traceable results against a no-demonstration baseline.

This is the minimum public product release. Recording alone does not satisfy it.

### M3: Cross-platform beta

Certify Windows and macOS profiles, improve browser integrations, add team review, harden storage/cost controls, and complete the beta evaluation suite.

Ship installers, update paths, permission onboarding, and per-platform compatibility reports. Do not advertise universal application coverage.

### M4: Training adapters and advanced exports

Deliver at least one genuinely working configured model-training adapter, provider lifecycle tracking, model artifacts, evaluation gates, and rollback metadata.

Other formats, native mobile capabilities, public dataset publishing, and advanced compositor adapters follow only after their dependencies and rights controls are validated.

## 22. Release, operations, and definition of done

Use existing CI plus Rust tests for broker changes and application/browser tests for Training. Include schema conformance, security scanning, capture fixtures, and CLI/MCP contract tests.

Deploy behind feature flags with migrations safe under concurrent startup. Keep capture and public release disabled by default. Test rollback without weakening authorization or corrupting dataset versions.

Publish code-signed desktop artifacts where distribution supports it; disclose any unsigned preview builds. Bundle required components and verify checksums. Do not overwrite an incompatible existing W3BS CLI silently.

Health checks must cover worker liveness, database access, artifact storage, queue progress, and cancellation processing, not merely the web server returning HTTP 200.

Documentation must include a one-task tutorial, privacy guide, capability matrix, importer specification, consumer setup, cost controls, deletion limitations, and disaster recovery.

The release is done only when:

- Both recorder and consumer experiences work end to end.
- The first-run path works without cloud inference.
- Human review and deletion remain available with automation disabled.
- Dataset provenance, purpose approvals, and revocation survive import/export and sync.
- At least one real agent consumes the data through the published interface.
- Security, performance, compatibility, and usefulness claims match recorded test evidence.

## 23. Main risks and explicit decisions

Passive desktop capture is the largest platform risk. Address it before building broad compatibility claims or a large hosted pipeline.

Low-quality labels can make a larger dataset worse. Keep uncertainty visible, prioritize review, and admit data per consumer profile.

UI drift can invalidate demonstrations. Track application versions, detect stale targets, and require current-state verification during execution.

All-day recordings can expose information unrelated to a task. Default to narrow scope, local processing, limited retention, and explicit release.

A new app must not become a stack rewrite. Preserve the Rust broker, existing W3BS host and identity, and current production database; add the Training interfaces behind stable contracts.

The product promise is not "watch everything and become perfect." It is "capture what is permitted, make it easy to label, and prove whether approved examples help an agent do the next task."

## 24. Source references and status

Sources were checked on October 15, 2026. Requirements, targets, interface names, and future resource classes in this PRD are proposed unless explicitly identified as existing.

[S1] W3BS Agentic Desktop Layer:
https://w3bs.org/desktop

[S2] Profullstack stack blueprint:
https://profullstack.com/stack

[S3] Existing Rust desktop workspace:
https://github.com/profullstack/w3bs.org/blob/main/desktop/Cargo.toml
Inspected blob: 1249a7b7413962398ae3a0a23ce1416abe9cef1c

[S4] Existing W3BS application manifest:
https://github.com/profullstack/w3bs.org/blob/main/package.json
Inspected blob: 5a260069936955b2b6aafb4325e2bd66ca9a7120

[S5] Existing W3BS repository architecture:
https://github.com/profullstack/w3bs.org/blob/main/README.md
Inspected blob: fc20306ae1e0d0c0885b52938b7c73e66a16978d

[S6] XDG InputCapture:
https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.InputCapture.html

[S7] XDG ScreenCast:
https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html

[S8] Windows UI Automation events:
https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-eventsoverview

[S9] Electron security guidance:
https://www.electronjs.org/docs/latest/tutorial/security

[S10] Browser native messaging:
https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging

[S11] AT-SPI API reference:
https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/
