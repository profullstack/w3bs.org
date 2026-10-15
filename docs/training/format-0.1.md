# W3BS Demonstration Dataset Format 0.1 (community draft)

Status: community draft. This is **not** a ratified W3BS standard. It is the
contract implemented by `training/schema/format.mjs`; the JSON Schema files in
`training/schema/json/` are generated from that module and published at
`https://w3bs.org/schemas/training/0.1/<record>.json` once the hosted app ships.

Valid and invalid examples live in `training/fixtures/valid` and
`training/fixtures/invalid`, each with a one-line reason.

## Principles

- **Observed and inferred are different things.** Every action carries a
  `source` (`human`, `device_event`, `application_event`, `deterministic_rule`,
  `model_inference`, `imported`) and a `status` (`proposed`, `confirmed`,
  `rejected`). A model's confidence is not correctness.
- **Actor origin is separate:** `human`, `agent`, `system`, `mixed`, `unknown`.
- **Gaps are records.** Capture denial, protected surfaces, dropped frames and
  missing origin evidence appear in `session.gaps`. Nothing fills them in.
- **No text values by default.** A text edit keeps the operation and, at most,
  a placeholder (`text_placeholder`). `{{name}}` placeholders name an episode
  parameter.
- **Nanosecond counters are decimal strings** (`"18025000000"`), because they
  exceed JavaScript's safe integer range.
- **Records are strict.** Unknown fields are refused, not ignored.

## Records

| Record              | Key fields                                                                                                                                                                                                                                                              |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `session`           | `session_id`, `capture_profile`, `clock` (wall-clock anchor + monotonic anchor + uncertainty), `device` (OS, desktop, locale, layout, displays with scale), `scope` (apps, origins, `text_values`), `capture_grant`, `adapters`, `gaps`                                 |
| `episode`           | `episode_id`, `goal` (label), `segments` (session + ns range; may be noncontiguous), `steps`, `parameters`, `prerequisites`, `outcome` (label + `evidence_type`), optional human `explanation`, `workflow_family`, `parent_episode_id`, `capture_profile`               |
| `observation`       | `observation_id`, `session_id`, `monotonic_ns`, `kind` (keyframe, crop, semantic, video_segment), `artifact_id`, `semantic` (app, window, focused target, origin), `display`, `pre_action_verified`, `visibility` (masked regions)                                      |
| `action` (one step) | `step_id`, `session_id`, `sequence`, `observation_before`, `observation_after`, `action` (`type`, `origin`, `source`, `status`, `target`, `coordinate_space`, `arguments`, `monotonic_ns`), `alignment` (`verified`/`uncertain`/`missing` + evidence), `raw_event_refs` |
| `label_revision`    | `revision_id`, `subject`, `field_path`, `value`, `source`, `status`, `evidence`, `author`, `confidence`, `created_at`, `supersedes`                                                                                                                                     |
| `artifact`          | `artifact_id`, bundle-relative `path` under `artifacts/`, `media_type` (PNG, JPEG, WebP, WebM, MP4 only), `sha256`, `bytes`, `redacted`                                                                                                                                 |
| `dataset_version`   | `dataset_id`, `version`, immutable `episodes`, `label_revisions`, `transformations`, `splits`, `purposes`, `digest`                                                                                                                                                     |
| `consumer_run`      | `run_id`, `kind`, `consumer`, `dataset`, `condition` (`with_demonstrations`/`baseline`), `task`, `task_family`, `used_episodes`, `outcome`, `interventions`, `unsafe_attempts`, `elapsed_ms`, `cost_usd`                                                                |
| `rights`            | `declared_by`, `source_rights` (`own_capture`, `licensed`, `authorized`, `unknown`), `permitted_uses`, `license`, `notes`                                                                                                                                               |

`pre_action_verified` is true only when the frame came from the recorder's
buffer before the action's event time. A frame requested after a click never
qualifies, however similar it looks.

### Difference from the PRD's illustrative step

The PRD's example step carried `episode_id`, `outcome` and `privacy` inline.
In 0.1 a step belongs to a session and is referenced from episodes (a step can
appear in a split or merged episode without being copied). Outcome lives on the
episode, and privacy policy lives in the session scope and import report.

## Bundle

A bundle is a ZIP file, conventionally `*.w3bs-training.zip`:

```text
manifest.json        required: format, schema_version, bundle_id, producer, capture_profile, counts, privacy
sessions.jsonl
episodes.jsonl
observations.jsonl
actions.jsonl
labels.jsonl
artifacts.jsonl
artifacts/<file>     png, jpg, jpeg, webp, webm, mp4 only
splits.json
rights.json
DATASET.md
checksums.json       { "<path>": "<sha256>" }
```

Anything else is rejected. Bundles carry no credentials, executable loaders,
absolute paths or remote URLs to fetch. Exports rename artifacts to
`artifacts/<artifact_id>.<ext>`. Digests are computed after redaction.
`dataset_version.digest` covers ids and label revision ids, not content, so it
is not a searchable fingerprint of private content.

## Importer specification

Pipeline, in order: receive, quarantine (sealed), validate manifest and limits,
inspect content, apply privacy policy, normalize, preview, commit. A partial
import is never searchable or consumable.

| Check      | Rule                                                                                                                                                                                                                                                                                                                           |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Archive    | No encryption, ZIP64, symlinks, executable mode bits, duplicate names, absolute paths, `..`, backslashes or drive letters. At most 20,000 entries, 512 MiB per entry, 2 GiB total, 200:1 compression ratio. Declared sizes and CRC32 must match.                                                                               |
| Files      | Only the names above.                                                                                                                                                                                                                                                                                                          |
| Schema     | `schema_version` must be `0.1`; otherwise `UNSUPPORTED_SCHEMA_VERSION`. Every record validated strictly.                                                                                                                                                                                                                       |
| References | Sessions, observations, steps and artifacts must resolve; ids must be unique.                                                                                                                                                                                                                                                  |
| Artifacts  | Size and SHA-256 match `artifacts.jsonl`; magic bytes must match the declared media type (an HTML file named `.png` is refused).                                                                                                                                                                                               |
| Privacy    | Secret-shaped strings are replaced with `[redacted:<kind>]` and raise a blocking warning; text values are removed unless both the bundle and the session scope allow them and the importer passes `--allow-text`; URL query strings and fragments are dropped; password-manager and sign-in contexts raise a blocking warning. |
| Labels     | Every imported goal and outcome becomes `proposed`. Only a person in the receiving store confirms.                                                                                                                                                                                                                             |
| Rights     | `rights.json`, or the uploader's `--rights-file`, or `unknown`. Unknown rights block external retrieval, model training and publication, not private review.                                                                                                                                                                   |
| Tombstones | An id deleted from this store cannot be re-imported.                                                                                                                                                                                                                                                                           |

### JSON/JSONL mapping

```json
{
  "timestamp": "ts",
  "timestamp_unit": "ms",
  "action_type": "event",
  "action_type_map": { "press": "click" },
  "target_name": "label",
  "target_role": "role",
  "app": "app",
  "window": "title",
  "url": "url",
  "x": "x",
  "y": "y",
  "keys": "keys",
  "episode_key": "task",
  "goal": "goal",
  "outcome": "result",
  "origin": "actor"
}
```

`timestamp_unit` is `s`, `ms`, `us`, `ns` or `iso`. Dotted paths reach nested
fields. Unmappable rows are counted by reason, and the preview lists what could
not be inferred. Without `episode_key`, episodes come from idle-gap
segmentation. Imported events are `source: imported`, `status: proposed`,
alignment `missing`, so they never qualify for action supervision.

### Video

`.webm` or `.mp4`, optionally with annotations
(`{"start_ms":0,"end_ms":4000,"goal":"...","outcome":"success"}` per line).
Video enters the `visual-review` profile and has no device-observed actions.

## Capture profiles

| Profile                  | Meaning                                                                                                                  |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| `visual-review`          | Frames or video a person can review. No claim about actions.                                                             |
| `semantic-demonstration` | Steps with UI targets, not necessarily verified before/after frames.                                                     |
| `action-supervision`     | Device-observed human actions with verified alignment and pre-action frames. Only these steps enter next-action exports. |

A dataset can be useful at a lower profile; it is never relabelled upward.
