//! Builds a W3BS Demonstration Dataset Format 0.1 bundle (`docs/training/format-0.1.md`) from
//! a finished recording, entirely in memory. The bundle is a ZIP of stored entries: frames are
//! PNG (already compressed) and nothing is written to disk here.
//!
//! The JavaScript importer in `training/` is the validator of record; a golden bundle produced by
//! the tests below is imported by `test/training-recorder.test.mjs` so the two cannot drift.

use crate::util::sha256_hex;
use serde_json::{json, Value};

pub const SCHEMA_VERSION: &str = "0.1";

#[derive(Clone, Debug)]
pub struct Frame {
    pub observation_id: String,
    pub artifact_id: String,
    pub ns: u64,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub app: String,
    pub title: String,
}

#[derive(Clone, Debug)]
pub struct FocusStep {
    pub step_id: String,
    pub ns: u64,
    pub app: String,
    pub title: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// `unknown` unless an agent held the input lease (`mixed`); see the capability matrix.
    pub origin: &'static str,
}

#[derive(Clone, Debug)]
pub struct Gap {
    pub kind: &'static str,
    pub from_ns: u64,
    pub to_ns: Option<u64>,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct Display {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// Everything the bundle needs, with no clocks or randomness, so tests can pin the output.
#[derive(Clone, Debug)]
pub struct RecordingData {
    pub session_id: String,
    pub bundle_id: String,
    pub teach: bool,
    pub goal: Option<String>,
    pub applications: Vec<String>,
    pub started_ms: u64,
    pub ended_ns: u64,
    pub tick_ns: u64,
    pub frames: Vec<Frame>,
    pub steps: Vec<FocusStep>,
    pub gaps: Vec<Gap>,
    pub marks: Vec<u64>,
    pub explanations: Vec<String>,
    /// `success`, `partial`, `failure`, `interrupted` or `unknown`, as the person said at finish.
    pub outcome: Option<String>,
    pub interrupted: bool,
    pub displays: Vec<Display>,
    pub adapter: String,
    pub producer_version: String,
    pub os: String,
    pub desktop: String,
    pub revocation_generation: u64,
    /// Episode ids, in order; one per segment (teach mode: exactly one).
    pub episode_ids: Vec<String>,
}

/// ISO 8601 UTC with milliseconds, from Unix milliseconds (civil-from-days, no time crate).
pub fn iso(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, rem % 3600 / 60, rem % 60, ms % 1000)
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Episode boundaries: teach mode is one episode; observe mode splits at the person's marks.
pub fn segments(d: &RecordingData) -> Vec<(u64, u64)> {
    let mut cuts: Vec<u64> = if d.teach { vec![] } else { d.marks.iter().copied().filter(|m| *m > 0 && *m < d.ended_ns).collect() };
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = vec![];
    let mut from = 0;
    for c in cuts {
        out.push((from, c));
        from = c;
    }
    out.push((from, d.ended_ns));
    // Keep segments with evidence; if none has any, keep the whole recording so the person can
    // still see it (and discard it) in review.
    let busy: Vec<(u64, u64)> = out
        .iter()
        .copied()
        .filter(|(a, b)| d.frames.iter().any(|f| f.ns >= *a && f.ns <= *b) || d.steps.iter().any(|s| s.ns >= *a && s.ns <= *b))
        .collect();
    if busy.is_empty() { vec![(0, d.ended_ns)] } else { busy }
}

pub fn build(d: &RecordingData) -> Vec<u8> {
    let ns = |n: u64| n.to_string();
    let wall = |n: u64| iso(d.started_ms + n / 1_000_000);
    let session = json!({
        "schema_version": SCHEMA_VERSION,
        "session_id": d.session_id,
        "capture_profile": "visual-review",
        "started_at": iso(d.started_ms),
        "ended_at": wall(d.ended_ns),
        "clock": {"wall_clock_anchor": iso(d.started_ms), "monotonic_anchor_ns": "0", "sync_uncertainty_ns": ns(d.tick_ns)},
        "device": {
            "os": clip(&d.os, 80),
            "desktop": clip(&d.desktop, 80),
            "displays": d.displays.iter().take(16).map(|x| json!({"id": clip(&x.id, 40), "x": x.x, "y": x.y, "width": x.width, "height": x.height, "scale": x.scale})).collect::<Vec<_>>(),
        },
        "scope": {"applications": d.applications.iter().take(200).map(|a| clip(a, 200)).collect::<Vec<_>>(), "origins": [], "text_values": false},
        "capture_grant": {"grant_id": format!("grant_{}", &d.session_id[4..]), "purpose": if d.teach { "teach this task" } else { "observe my work" }, "revocation_generation": d.revocation_generation},
        "adapters": [{"name": clip(&d.adapter, 80), "version": clip(&d.producer_version, 40)}],
        "gaps": d.gaps.iter().map(|g| {
            let mut v = json!({"kind": g.kind, "from_ns": ns(g.from_ns), "detail": clip(&g.detail, 500)});
            if let Some(t) = g.to_ns { v["to_ns"] = json!(ns(t)); }
            v
        }).collect::<Vec<_>>(),
    });
    let observations: Vec<Value> = d
        .frames
        .iter()
        .map(|f| {
            json!({
                "schema_version": SCHEMA_VERSION, "observation_id": f.observation_id, "session_id": d.session_id,
                "monotonic_ns": ns(f.ns), "kind": "crop", "artifact_id": f.artifact_id,
                "semantic": {"app": clip(&f.app, 200), "window": clip(&f.title, 400)},
                "display": {"width": f.width, "height": f.height, "scale": 1},
                // Frames are polled, never taken from a buffer before an input event.
                "pre_action_verified": false,
                "visibility": {"masked_regions": [], "restricted": false},
            })
        })
        .collect();
    let actions: Vec<Value> = d
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let evidence: Vec<String> = [s.before.clone(), s.after.clone()].into_iter().flatten().collect();
            let mut v = json!({
                "schema_version": SCHEMA_VERSION, "step_id": s.step_id, "session_id": d.session_id, "sequence": i,
                "action": {
                    "type": "focus", "origin": s.origin, "source": "application_event", "status": "confirmed",
                    "target": {"app": clip(&s.app, 200), "window": clip(&s.title, 400), "source": "window_manager"},
                    "coordinate_space": "none", "arguments": {}, "monotonic_ns": ns(s.ns),
                },
                // Focus is polled, so before/after frames bracket the change only within one tick.
                "alignment": {"status": "uncertain", "evidence": evidence},
                "raw_event_refs": [],
            });
            if let Some(b) = &s.before { v["observation_before"] = json!(b); }
            if let Some(a) = &s.after { v["observation_after"] = json!(a); }
            v
        })
        .collect();
    let artifacts: Vec<Value> = d
        .frames
        .iter()
        .map(|f| json!({"schema_version": SCHEMA_VERSION, "artifact_id": f.artifact_id, "path": format!("artifacts/{}.png", f.artifact_id), "media_type": "image/png", "sha256": sha256_hex(&f.png), "bytes": f.png.len()}))
        .collect();
    let segs = segments(d);
    let last = segs.len() - 1;
    let episodes: Vec<Value> = segs
        .iter()
        .enumerate()
        .map(|(i, (a, b))| {
            let apps: Vec<&str> = {
                let mut v: Vec<&str> = d.frames.iter().filter(|f| f.ns >= *a && f.ns <= *b).map(|f| f.app.as_str()).collect();
                v.dedup();
                v.truncate(2);
                v
            };
            let goal = match (&d.goal, d.teach) {
                (Some(g), true) => json!({"value": clip(g, 500), "source": "human", "status": "confirmed", "evidence": []}),
                _ => json!({"value": if apps.is_empty() { "Untitled task".to_string() } else { format!("Work in {}", apps.join(" and ")) }, "source": "deterministic_rule", "status": "proposed", "evidence": []}),
            };
            let outcome = match (&d.outcome, d.interrupted, i == last) {
                (_, true, true) => json!({"value": "interrupted", "source": "deterministic_rule", "status": "proposed", "evidence": [], "evidence_type": "none"}),
                (Some(o), false, true) => json!({"value": o, "source": "human", "status": "confirmed", "evidence": [], "evidence_type": "human_assertion"}),
                _ => json!({"value": "unknown", "source": "deterministic_rule", "status": "proposed", "evidence": [], "evidence_type": "none"}),
            };
            let mut e = json!({
                "schema_version": SCHEMA_VERSION, "episode_id": d.episode_ids[i.min(d.episode_ids.len() - 1)],
                "goal": goal,
                "segments": [{"session_id": d.session_id, "from_ns": ns(*a), "to_ns": ns(*b)}],
                "steps": d.steps.iter().filter(|s| s.ns >= *a && s.ns <= *b).map(|s| s.step_id.clone()).collect::<Vec<_>>(),
                "parameters": [], "prerequisites": [],
                "outcome": outcome,
                "capture_profile": "visual-review",
            });
            if d.teach && !d.explanations.is_empty() {
                e["explanation"] = json!({"text": clip(&d.explanations.join("\n\n"), 8000), "author": "human"});
            }
            e
        })
        .collect();
    let manifest = json!({
        "format": "w3bs-demonstration-dataset", "schema_version": SCHEMA_VERSION, "bundle_id": d.bundle_id,
        "created_at": wall(d.ended_ns), "producer": {"name": "w3bs-desktop", "version": clip(&d.producer_version, 40)},
        "capture_profile": "visual-review",
        "counts": {"sessions": 1, "episodes": episodes.len(), "observations": observations.len(), "actions": actions.len(), "labels": 0, "artifacts": artifacts.len()},
        "privacy": {"text_values_included": false, "redaction_applied": false},
    });
    let rights = json!({"declared_by": "local-recorder", "source_rights": "own_capture", "permitted_uses": ["human_review", "private_retrieval", "evaluation"]});
    let jsonl = |v: &[Value]| v.iter().map(|x| x.to_string() + "\n").collect::<String>().into_bytes();
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("manifest.json".into(), manifest.to_string().into_bytes()),
        ("sessions.jsonl".into(), jsonl(&[session])),
        ("episodes.jsonl".into(), jsonl(&episodes)),
        ("observations.jsonl".into(), jsonl(&observations)),
        ("actions.jsonl".into(), jsonl(&actions)),
        ("artifacts.jsonl".into(), jsonl(&artifacts)),
        ("rights.json".into(), rights.to_string().into_bytes()),
    ];
    for f in &d.frames {
        files.push((format!("artifacts/{}.png", f.artifact_id), f.png.clone()));
    }
    let sums: serde_json::Map<String, Value> = files.iter().map(|(n, c)| (n.clone(), json!(sha256_hex(c)))).collect();
    files.push(("checksums.json".into(), Value::Object(sums).to_string().into_bytes()));
    zip_stored(&files)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xedb8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    !crc
}

/// A ZIP of stored (uncompressed) entries with fixed timestamps and 0644 modes.
pub fn zip_stored(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let n = name.as_bytes();
        let le16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
        let le32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
        le32(&mut out, 0x0403_4b50);
        for x in [20u16, 0x0800, 0, 0, 0x21] {
            le16(&mut out, x);
        }
        le32(&mut out, crc);
        le32(&mut out, data.len() as u32);
        le32(&mut out, data.len() as u32);
        le16(&mut out, n.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(n);
        out.extend_from_slice(data);
        le32(&mut central, 0x0201_4b50);
        for x in [(3 << 8) | 20u16, 20, 0x0800, 0, 0, 0x21] {
            le16(&mut central, x);
        }
        le32(&mut central, crc);
        le32(&mut central, data.len() as u32);
        le32(&mut central, data.len() as u32);
        le16(&mut central, n.len() as u16);
        for _ in 0..4 {
            le16(&mut central, 0);
        }
        le32(&mut central, 0o100644 << 16);
        le32(&mut central, offset);
        central.extend_from_slice(n);
    }
    let cd_offset = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn png(w: u32, h: u32, shade: u8) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&vec![shade; (w * h) as usize]).unwrap();
        }
        out
    }

    /// A fixed teach-mode recording: two apps, three focus steps, one excluded-scope gap.
    pub fn golden() -> RecordingData {
        let frame = |i: u32, ns: u64, app: &str, title: &str| Frame { observation_id: format!("obs_rec{i}"), artifact_id: format!("art_rec{i}"), ns, png: png(4, 3, 40 * i as u8), width: 4, height: 3, app: app.into(), title: title.into() };
        let step = |i: u32, ns: u64, app: &str, title: &str, before: Option<&str>, after: &str| FocusStep { step_id: format!("step_rec{i}"), ns, app: app.into(), title: title.into(), before: before.map(Into::into), after: Some(after.into()), origin: "unknown" };
        RecordingData {
            session_id: "ses_rec1".into(),
            bundle_id: "bun_rec1".into(),
            teach: true,
            goal: Some("Prepare a release-announcement draft".into()),
            applications: vec!["org.mozilla.firefox".into(), "org.kde.kate".into()],
            started_ms: 1_760_544_000_000,
            ended_ns: 20_000_000_000,
            tick_ns: 500_000_000,
            frames: vec![
                frame(1, 500_000_000, "org.mozilla.firefox", "Releases - Firefox"),
                frame(2, 6_000_000_000, "org.kde.kate", "announcement.md - Kate"),
                frame(3, 12_000_000_000, "org.mozilla.firefox", "New release - Firefox"),
            ],
            steps: vec![
                step(1, 500_000_000, "org.mozilla.firefox", "Releases - Firefox", None, "obs_rec1"),
                step(2, 6_000_000_000, "org.kde.kate", "announcement.md - Kate", Some("obs_rec1"), "obs_rec2"),
                step(3, 12_000_000_000, "org.mozilla.firefox", "New release - Firefox", Some("obs_rec2"), "obs_rec3"),
            ],
            gaps: vec![Gap { kind: "excluded_scope", from_ns: 8_000_000_000, to_ns: Some(9_000_000_000), detail: "a window outside the approved applications was focused".into() }],
            marks: vec![],
            explanations: vec!["Copy the version line from the release page first.".into()],
            outcome: Some("success".into()),
            interrupted: false,
            displays: vec![Display { id: "DP-1".into(), x: 0.0, y: 0.0, width: 1920.0, height: 1080.0, scale: 1.0 }],
            adapter: "mock".into(),
            producer_version: "0.0.0-test".into(),
            os: "linux".into(),
            desktop: "KDE".into(),
            revocation_generation: 1,
            episode_ids: vec!["ep_rec1".into()],
        }
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_760_544_000_123), "2025-10-15T16:00:00.123Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn crc_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn observe_mode_splits_at_marks_and_drops_empty_segments() {
        let mut d = golden();
        d.teach = false;
        d.marks = vec![3_000_000_000, 4_000_000_000, 10_000_000_000];
        d.episode_ids = vec!["ep_a".into(), "ep_b".into(), "ep_c".into(), "ep_d".into()];
        assert_eq!(segments(&d), vec![(0, 3_000_000_000), (4_000_000_000, 10_000_000_000), (10_000_000_000, 20_000_000_000)]);
    }

    /// Writes the golden bundle with `W3BS_UPDATE_GOLDEN=1`; otherwise checks it is unchanged.
    /// `test/training-recorder.test.mjs` imports the same file with the JavaScript validator.
    #[test]
    fn golden_bundle_is_stable() {
        let bytes = build(&golden());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../training/fixtures/recorder/teach.w3bs-training.zip");
        if std::env::var("W3BS_UPDATE_GOLDEN").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &bytes).unwrap();
        }
        let committed = std::fs::read(&path).expect("golden bundle missing: run with W3BS_UPDATE_GOLDEN=1");
        assert!(committed == bytes, "the recorder's bundle changed; regenerate with W3BS_UPDATE_GOLDEN=1 and re-run the JS test");
    }
}
