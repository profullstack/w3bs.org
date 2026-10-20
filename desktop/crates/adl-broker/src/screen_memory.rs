//! Screen memory: the fast path for finding a target an agent has found before.
//!
//! An agent that works from screenshots pays for a full model pass every time it looks for the
//! same button. Following the detector + vision-model split in arXiv 2609.02393 (Schneier,
//! 2026-10-19), the agent's own successful finds become labels: after it locates a target the slow
//! way it saves a small grayscale patch around it (`remember`), and later `locate` looks for that
//! patch on a *fresh* capture with zero-mean normalized cross-correlation. A confident, unambiguous
//! match returns a point in that new capture; anything else sends the agent back to the slow path,
//! and its next `remember` refreshes the patch, so the memory recovers when the UI changes.
//!
//! Safety properties:
//! - Old coordinates are never replayed: every answer is a point in a capture taken just now.
//! - Two near-equal matches are refused as ambiguous, so it never picks one of two identical buttons.
//! - Featureless patches (a blank field, a flat background) are refused at `remember`.
//! - Patches are small (at most 128x128 pixels), per agent, in a 0600 file the person can list and
//!   clear; removing an agent deletes its patches.

use crate::util::now_ms;
use adl_schema::{AdlError, Result, ScreenMemoryEntry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Patches per agent; the least recently used goes first.
pub const MAX_PER_AGENT: usize = 128;
/// Patch half-size bounds in capture pixels (patch side = 2 * radius).
pub const MIN_RADIUS: u32 = 12;
pub const MAX_RADIUS: u32 = 64;
pub const DEFAULT_RADIUS: u32 = 24;
/// Default and allowed range for the confidence a match needs.
pub const DEFAULT_MIN_SCORE: f32 = 0.90;
/// Below this pixel standard deviation a patch has nothing to match on.
const MIN_STDDEV: f64 = 6.0;
/// A runner-up this close to the best (and above the bar) makes the answer ambiguous.
const AMBIGUITY_MARGIN: f32 = 0.03;

/// An 8-bit grayscale image.
#[derive(Clone, Debug, PartialEq)]
pub struct Gray {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Gray {
    /// Decode a capture PNG of any colour type to grayscale.
    pub fn from_png(png_bytes: &[u8]) -> Result<Self> {
        let mut dec = png::Decoder::new(png_bytes);
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec.read_info().map_err(|e| AdlError::internal(format!("decode capture: {e}")))?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| AdlError::internal(format!("decode capture: {e}")))?;
        let (w, h, ch, stride) = (info.width, info.height, info.color_type.samples(), info.line_size);
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h as usize {
            let row = &buf[y * stride..y * stride + w as usize * ch];
            for p in row.chunks_exact(ch) {
                px.push(match ch {
                    1 | 2 => p[0],
                    _ => ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8,
                });
            }
        }
        Ok(Self { w, h, px })
    }

    fn at(&self, x: u32, y: u32) -> u8 {
        self.px[(y * self.w + x) as usize]
    }

    /// The `2r x 2r` patch centred on (cx, cy), if it lies inside the image.
    pub fn crop(&self, cx: u32, cy: u32, r: u32) -> Option<Gray> {
        if cx < r || cy < r || cx + r > self.w || cy + r > self.h {
            return None;
        }
        let (x0, y0, side) = (cx - r, cy - r, 2 * r);
        let mut px = Vec::with_capacity((side * side) as usize);
        for y in y0..y0 + side {
            px.extend_from_slice(&self.px[(y * self.w + x0) as usize..(y * self.w + x0 + side) as usize]);
        }
        Some(Gray { w: side, h: side, px })
    }

    pub fn stddev(&self) -> f64 {
        let n = self.px.len() as f64;
        let mean = self.px.iter().map(|&v| v as f64).sum::<f64>() / n;
        (self.px.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n).sqrt()
    }

    /// Box-filter downscale by an integer factor.
    fn downscale(&self, f: u32) -> Gray {
        if f <= 1 {
            return self.clone();
        }
        let (w, h) = (self.w / f, self.h / f);
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut s = 0u32;
                for dy in 0..f {
                    for dx in 0..f {
                        s += self.at(x * f + dx, y * f + dy) as u32;
                    }
                }
                px.push((s / (f * f)) as u8);
            }
        }
        Gray { w, h, px }
    }

    /// Nearest-neighbour resize (a patch saved on a display with another scale).
    pub fn resize(&self, w: u32, h: u32) -> Gray {
        let (w, h) = (w.max(1), h.max(1));
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                px.push(self.at((x * self.w / w).min(self.w - 1), (y * self.h / h).min(self.h - 1)));
            }
        }
        Gray { w, h, px }
    }
}

/// Integral images of an image and its squares, for O(1) window sums.
struct Integral {
    w: usize,
    s: Vec<f64>,
    s2: Vec<f64>,
}

impl Integral {
    fn new(g: &Gray) -> Self {
        let (w, h) = (g.w as usize + 1, g.h as usize + 1);
        let (mut s, mut s2) = (vec![0.0; w * h], vec![0.0; w * h]);
        for y in 1..h {
            let (mut row, mut row2) = (0.0, 0.0);
            for x in 1..w {
                let v = g.px[(y - 1) * g.w as usize + x - 1] as f64;
                row += v;
                row2 += v * v;
                s[y * w + x] = s[(y - 1) * w + x] + row;
                s2[y * w + x] = s2[(y - 1) * w + x] + row2;
            }
        }
        Self { w, s, s2 }
    }

    fn window(&self, x: usize, y: usize, tw: usize, th: usize) -> (f64, f64) {
        let w = self.w;
        let f = |a: &Vec<f64>| a[(y + th) * w + x + tw] - a[y * w + x + tw] - a[(y + th) * w + x] + a[y * w + x];
        (f(&self.s), f(&self.s2))
    }
}

/// A template centred and normalised for correlation.
struct Prepared {
    w: usize,
    h: usize,
    centred: Vec<f64>,
    norm: f64,
}

impl Prepared {
    fn new(t: &Gray) -> Option<Self> {
        let n = t.px.len() as f64;
        let mean = t.px.iter().map(|&v| v as f64).sum::<f64>() / n;
        let centred: Vec<f64> = t.px.iter().map(|&v| v as f64 - mean).collect();
        let norm = centred.iter().map(|v| v * v).sum::<f64>().sqrt();
        (norm > 1e-6).then_some(Self { w: t.w as usize, h: t.h as usize, centred, norm })
    }

    /// Zero-mean normalized cross-correlation at (x, y), in [-1, 1]. Because the template is
    /// centred, sum(t * (I - mean I)) = sum(t * I), so only the image's window variance is needed.
    fn score(&self, img: &Gray, ii: &Integral, x: usize, y: usize) -> f32 {
        let n = (self.w * self.h) as f64;
        let (s, s2) = ii.window(x, y, self.w, self.h);
        let var = s2 - s * s / n;
        if var <= 1e-6 {
            return 0.0;
        }
        let iw = img.w as usize;
        let mut dot = 0.0;
        for ty in 0..self.h {
            let row = (y + ty) * iw + x;
            let trow = ty * self.w;
            for tx in 0..self.w {
                dot += self.centred[trow + tx] * img.px[row + tx] as f64;
            }
        }
        (dot / (self.norm * var.sqrt())) as f32
    }
}

/// Where a template matched in an image: the centre of the best window, its score, and the best
/// score of any other place (to tell a unique match from one of several look-alikes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Match {
    pub x: f64,
    pub y: f64,
    pub score: f32,
    pub runner_up: f32,
}

/// Find `tpl` in `img`: a full scan on a downscaled pyramid level, then a local full-resolution
/// refinement of the few best coarse candidates. None when the template does not fit or has no
/// detail.
pub fn find(img: &Gray, tpl: &Gray) -> Option<Match> {
    if tpl.w > img.w || tpl.h > img.h {
        return None;
    }
    let side = tpl.w.min(tpl.h);
    let f = if side >= 32 { 4 } else if side >= 16 { 2 } else { 1 };
    let (ci, ct) = (img.downscale(f), tpl.downscale(f));
    let cp = Prepared::new(&ct)?;
    let cii = Integral::new(&ci);
    let (cw, ch) = (ci.w as usize - cp.w + 1, ci.h as usize - cp.h + 1);
    let mut coarse: Vec<(f32, usize, usize)> = Vec::new();
    for y in 0..ch {
        for x in 0..cw {
            let s = cp.score(&ci, &cii, x, y);
            if s > 0.4 {
                coarse.push((s, x, y));
            }
        }
    }
    coarse.sort_by(|a, b| b.0.total_cmp(&a.0));
    // Non-maximum suppression: keep up to 6 candidates at least half a template apart.
    let sep = (cp.w.max(cp.h) / 2).max(1) as i64;
    let mut picks: Vec<(usize, usize)> = Vec::new();
    for &(_, x, y) in &coarse {
        if picks.iter().all(|&(px, py)| (px as i64 - x as i64).abs() >= sep || (py as i64 - y as i64).abs() >= sep) {
            picks.push((x, y));
            if picks.len() == 6 {
                break;
            }
        }
    }
    let fp = Prepared::new(tpl)?;
    let fii = Integral::new(img);
    let (maxx, maxy) = ((img.w - tpl.w) as i64, (img.h - tpl.h) as i64);
    let reach = 2 * f as i64;
    let mut found: Vec<(f32, usize, usize)> = picks
        .iter()
        .map(|&(cx, cy)| {
            let (bx, by) = (cx as i64 * f as i64, cy as i64 * f as i64);
            let mut best = (f32::MIN, 0usize, 0usize);
            for y in (by - reach).max(0)..=(by + reach).min(maxy) {
                for x in (bx - reach).max(0)..=(bx + reach).min(maxx) {
                    let s = fp.score(img, &fii, x as usize, y as usize);
                    if s > best.0 {
                        best = (s, x as usize, y as usize);
                    }
                }
            }
            best
        })
        .filter(|b| b.0 > f32::MIN)
        .collect();
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let &(score, x, y) = found.first()?;
    let far = (tpl.w.max(tpl.h) / 2) as i64;
    let runner_up = found
        .iter()
        .skip(1)
        .find(|&&(_, ox, oy)| (ox as i64 - x as i64).abs() >= far || (oy as i64 - y as i64).abs() >= far)
        .map_or(0.0, |o| o.0);
    Some(Match { x: x as f64 + tpl.w as f64 / 2.0, y: y as f64 + tpl.h as f64 / 2.0, score, runner_up })
}

/// How a match is routed: confident and unique, or back to the slow path.
pub fn accept(m: &Match, min_score: f32) -> (bool, bool) {
    let ambiguous = m.runner_up >= min_score && m.score - m.runner_up < AMBIGUITY_MARGIN;
    (m.score >= min_score && !ambiguous, ambiguous)
}

mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Template {
    pub label: String,
    pub w: u32,
    pub h: u32,
    #[serde(with = "b64")]
    pub px: Vec<u8>,
    /// Capture pixels per logical pixel when it was saved.
    pub scale: f64,
    pub display: String,
    pub saved_at_ms: u64,
    pub last_used_ms: u64,
    pub hits: u64,
    pub misses: u64,
}

impl Template {
    pub fn gray(&self) -> Gray {
        Gray { w: self.w, h: self.h, px: self.px.clone() }
    }

    fn entry(&self, agent: &str) -> ScreenMemoryEntry {
        ScreenMemoryEntry {
            agent: agent.into(),
            label: self.label.clone(),
            width: self.w,
            height: self.h,
            display: self.display.clone(),
            saved_at_ms: self.saved_at_ms,
            last_used_ms: self.last_used_ms,
            hits: self.hits,
            misses: self.misses,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    v: u32,
    agents: HashMap<String, Vec<Template>>,
}

/// Saved patches, per agent. Written atomically with mode 0600.
pub struct ScreenMemory {
    path: Option<PathBuf>,
    file: Mutex<File>,
}

pub fn check_label(label: &str) -> Result<String> {
    let l = label.trim();
    if l.is_empty() || l.chars().count() > 80 {
        return Err(AdlError::invalid("label must be 1-80 characters, e.g. \"Firefox: Sign in button\""));
    }
    Ok(l.to_string())
}

impl ScreenMemory {
    /// In-memory (tests, and brokers without a state directory).
    pub fn memory() -> Self {
        Self { path: None, file: Mutex::new(File { v: 1, ..Default::default() }) }
    }

    pub fn open(path: PathBuf) -> Result<Self> {
        let file = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| AdlError::internal(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File { v: 1, ..Default::default() },
            Err(e) => return Err(AdlError::internal(format!("{}: {e}", path.display()))),
        };
        Ok(Self { path: Some(path), file: Mutex::new(file) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        self.file.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, f: &File) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let io = |e: std::io::Error| AdlError::internal(format!("{}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut out = crate::util::private_file().write(true).create(true).truncate(true).open(&tmp).map_err(io)?;
            out.write_all(&serde_json::to_vec(f).unwrap()).map_err(io)?;
            out.sync_all().map_err(io)?;
        }
        std::fs::rename(&tmp, path).map_err(io)
    }

    /// Save (or replace) the patch for `label`. The caller has checked the patch has detail.
    pub fn remember(&self, agent: &str, label: &str, patch: &Gray, scale: f64, display: &str) -> Result<ScreenMemoryEntry> {
        let now = now_ms();
        let mut f = self.lock();
        let list = f.agents.entry(agent.into()).or_default();
        let (hits, misses) = list.iter().find(|t| t.label == label).map_or((0, 0), |t| (t.hits, t.misses));
        list.retain(|t| t.label != label);
        if list.len() >= MAX_PER_AGENT {
            list.sort_by_key(|t| std::cmp::Reverse(t.last_used_ms.max(t.saved_at_ms)));
            list.truncate(MAX_PER_AGENT - 1);
        }
        let t = Template { label: label.into(), w: patch.w, h: patch.h, px: patch.px.clone(), scale, display: display.into(), saved_at_ms: now, last_used_ms: now, hits, misses };
        let entry = t.entry(agent);
        list.push(t);
        self.save(&f)?;
        Ok(entry)
    }

    pub fn get(&self, agent: &str, label: &str) -> Option<Template> {
        self.lock().agents.get(agent)?.iter().find(|t| t.label == label).cloned()
    }

    /// Count a locate as a hit or a miss (the person can see which patches have gone stale).
    pub fn record(&self, agent: &str, label: &str, hit: bool) {
        let mut f = self.lock();
        if let Some(t) = f.agents.get_mut(agent).and_then(|l| l.iter_mut().find(|t| t.label == label)) {
            t.last_used_ms = now_ms();
            if hit {
                t.hits += 1;
            } else {
                t.misses += 1;
            }
            let _ = self.save(&f);
        }
    }

    /// Patch metadata (never pixels), for one agent or all.
    pub fn list(&self, agent: Option<&str>) -> Vec<ScreenMemoryEntry> {
        let f = self.lock();
        let mut v: Vec<ScreenMemoryEntry> = f.agents.iter().filter(|(a, _)| agent.is_none_or(|x| x == a.as_str())).flat_map(|(a, l)| l.iter().map(|t| t.entry(a))).collect();
        v.sort_by(|a, b| (&a.agent, &a.label).cmp(&(&b.agent, &b.label)));
        v
    }

    /// Delete one label, or every patch of the agent. Returns how many went.
    pub fn forget(&self, agent: &str, label: Option<&str>) -> Result<usize> {
        let mut f = self.lock();
        let n = match label {
            None => f.agents.remove(agent).map_or(0, |l| l.len()),
            Some(l) => f.agents.get_mut(agent).map_or(0, |list| {
                let before = list.len();
                list.retain(|t| t.label != l);
                before - list.len()
            }),
        };
        if n > 0 {
            self.save(&f)?;
        }
        Ok(n)
    }
}

pub fn has_detail(t: &Gray) -> bool {
    t.stddev() >= MIN_STDDEV
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic textured image: a noisy background with a few distinct "widgets".
    fn scene(w: u32, h: u32) -> Gray {
        let mut px = Vec::with_capacity((w * h) as usize);
        let mut s: u32 = 12345;
        for _ in 0..w * h {
            s = s.wrapping_mul(1103515245).wrapping_add(12345);
            px.push(200 + ((s >> 16) % 24) as u8);
        }
        let mut g = Gray { w, h, px };
        for (x0, y0, v) in [(300u32, 200u32, 40u8), (900, 600, 90), (1500, 300, 140)] {
            if x0 + 120 > w || y0 + 40 > h {
                continue;
            }
            for y in y0..y0 + 40 {
                for x in x0..x0 + 120 {
                    let i = (y * w + x) as usize;
                    // A non-repeating pattern inside each widget (like text), so their insides differ.
                    let mut k = (x - x0).wrapping_mul(0x9E37_79B9) ^ (y - y0).wrapping_mul(0x85EB_CA6B) ^ v as u32;
                    k ^= k >> 15;
                    k = k.wrapping_mul(0x2C1B_3C6D);
                    k ^= k >> 12;
                    g.px[i] = v.wrapping_add((k % 80) as u8);
                }
            }
        }
        g
    }

    #[test]
    fn finds_a_patch_where_it_moved() {
        let a = scene(1920, 1080);
        let tpl = a.crop(960, 620, 24).unwrap();
        assert!(has_detail(&tpl));
        let m = find(&a, &tpl).unwrap();
        assert!((m.x - 960.0).abs() <= 1.0 && (m.y - 620.0).abs() <= 1.0, "{m:?}");
        assert!(m.score > 0.99);
        // The same widget drawn somewhere else (the window moved) is found there.
        let mut b = scene(1920, 1080);
        for y in 0..40 {
            for x in 0..120 {
                b.px[((100 + y) * 1920 + 1000 + x) as usize] = a.px[((600 + y) * 1920 + 900 + x) as usize];
                b.px[((600 + y) * 1920 + 900 + x) as usize] = 210;
            }
        }
        let m = find(&b, &tpl).unwrap();
        assert!((m.x - 1060.0).abs() <= 1.0 && (m.y - 120.0).abs() <= 1.0, "{m:?}");
        assert!(accept(&m, DEFAULT_MIN_SCORE).0, "{m:?}");
    }

    #[test]
    fn a_missing_target_is_not_accepted() {
        let a = scene(1920, 1080);
        let tpl = a.crop(960, 620, 24).unwrap();
        let mut b = a.clone();
        for y in 560..700 {
            for x in 860..1060 {
                b.px[(y * 1920 + x) as usize] = 210;
            }
        }
        assert!(find(&b, &tpl).is_none_or(|m| !accept(&m, DEFAULT_MIN_SCORE).0));
    }

    #[test]
    fn two_identical_targets_are_ambiguous() {
        let mut a = scene(1920, 1080);
        for y in 0..40 {
            for x in 0..120 {
                a.px[((900 + y) * 1920 + 200 + x) as usize] = a.px[((600 + y) * 1920 + 900 + x) as usize];
            }
        }
        let tpl = a.crop(960, 620, 24).unwrap();
        let m = find(&a, &tpl).unwrap();
        assert_eq!(accept(&m, DEFAULT_MIN_SCORE), (false, true), "{m:?}");
    }

    /// `cargo test --release -p adl-broker screen_memory -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_4k_locate() {
        let mut a = scene(3840, 2160);
        for y in 0..40 {
            for x in 0..120 {
                a.px[((1700 + y) * 3840 + 3000 + x) as usize] = a.px[((600 + y) * 3840 + 900 + x) as usize];
            }
        }
        let tpl = scene(3840, 2160).crop(960, 620, 24).unwrap();
        let t = std::time::Instant::now();
        let m = find(&a, &tpl).unwrap();
        eprintln!("4K locate: {:?} in {:?}", m, t.elapsed());
    }

    #[test]
    fn flat_patches_have_no_detail() {
        let g = Gray { w: 48, h: 48, px: vec![0xe0; 48 * 48] };
        assert!(!has_detail(&g));
        assert!(find(&scene(400, 300), &g).is_none());
    }

    #[test]
    fn memory_round_trips_and_evicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screen-memory.json");
        let m = ScreenMemory::open(path.clone()).unwrap();
        let p = scene(200, 200).crop(100, 100, 16).unwrap();
        m.remember("a1", "ok button", &p, 1.0, "DP-1").unwrap();
        m.record("a1", "ok button", true);
        let back = ScreenMemory::open(path).unwrap();
        let t = back.get("a1", "ok button").unwrap();
        assert_eq!(t.gray(), p);
        assert_eq!(t.hits, 1);
        for i in 0..MAX_PER_AGENT + 5 {
            back.remember("a1", &format!("l{i}"), &p, 1.0, "DP-1").unwrap();
        }
        assert_eq!(back.list(Some("a1")).len(), MAX_PER_AGENT);
        assert_eq!(back.forget("a1", None).unwrap(), MAX_PER_AGENT);
        assert!(back.list(None).is_empty());
    }
}
