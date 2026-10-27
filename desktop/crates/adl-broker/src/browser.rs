//! The browser channel: agents drive web pages through DevTools (CDP) instead of screenshots.
//!
//! An agent reads a page as an accessibility snapshot whose lines carry `uid`s (DevTools backend
//! node ids), then clicks, fills and types by uid. That is one round trip per step and needs no
//! image, no coordinates and no screen capture permission, which is why it is the primary path
//! for anything in a browser. Screenshots and OS input stay as the fallback for everything else.
//!
//! Safety: the same gates as desktop input. Observing needs the layer Enabled and the `browser`
//! grant; acting also needs the task's input lease (so the drive prompt, human steps, training
//! recordings, emergency stop and takeover all apply), honours `approval_required_for`
//! (`browser`, or one operation such as `browser_click`) and the per-minute action limit. Audit
//! keeps the operation and page id only, never typed text, values or page content.
//!
//! When DevTools cannot reach a browser, every call fails with `blocked` and
//! `details.browser_unavailable = true`. Nothing falls back on its own: the agent must say so
//! (and report a failed training run) rather than quietly switching to screenshots.

use crate::broker::{Broker, Principal};
use crate::cdp::{self, Cdp};
use adl_schema::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct BrowserInner {
    cdp: Option<Cdp>,
}

const MAX_SNAPSHOT_LINES: usize = 1500;
const MAX_TEXT: usize = 160;
const MAX_EVAL_BYTES: usize = 100_000;

fn trunc(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= n { s } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

fn ax_str(n: &Value, k: &str) -> String {
    match &n[k]["value"] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// Render `Accessibility.getFullAXTree` as an indented outline, one line per meaningful node:
/// `uid=123 button "Sign in" focused`. Unnamed generic containers and ignored nodes are folded
/// into their children; inline text boxes are dropped.
pub(crate) fn render_ax(nodes: &[Value], max_lines: usize) -> (String, usize) {
    use std::collections::HashMap;
    let by_id: HashMap<&str, &Value> = nodes.iter().filter_map(|n| n["nodeId"].as_str().map(|id| (id, n))).collect();
    let Some(root) = nodes.iter().find(|n| n["parentId"].is_null()).or(nodes.first()) else { return (String::new(), 0) };
    let mut out = String::new();
    let mut lines = 0usize;
    let mut omitted = 0usize;
    // (node, depth, parent's name)
    let mut stack: Vec<(&Value, usize, String)> = vec![(root, 0, String::new())];
    while let Some((n, depth, parent_name)) = stack.pop() {
        let role = ax_str(n, "role");
        let name = ax_str(n, "name");
        let ignored = n["ignored"].as_bool().unwrap_or(false);
        if role == "InlineTextBox" {
            continue;
        }
        let fold = ignored || (matches!(role.as_str(), "generic" | "none" | "LineBreak" | "presentation" | "") && name.trim().is_empty()) || (role == "StaticText" && (name.trim().is_empty() || name == parent_name));
        let mut child_depth = depth;
        if !fold {
            if lines < max_lines {
                let mut line = "  ".repeat(depth.min(40));
                if let Some(b) = n["backendDOMNodeId"].as_u64() {
                    line += &format!("uid={b} ");
                }
                if role == "StaticText" {
                    line += &format!("text {:?}", trunc(&name, MAX_TEXT));
                } else {
                    line += &role;
                    if !name.trim().is_empty() {
                        line += &format!(" {:?}", trunc(&name, MAX_TEXT));
                    }
                    let value = ax_str(n, "value");
                    if !value.is_empty() {
                        line += &format!(" value={:?}", trunc(&value, MAX_TEXT));
                    }
                    for p in n["properties"].as_array().into_iter().flatten() {
                        let pname = p["name"].as_str().unwrap_or("");
                        let pv = &p["value"]["value"];
                        match pname {
                            "focused" | "disabled" | "required" | "readonly" | "multiline" | "modal" if pv == &json!(true) => line += &format!(" {pname}"),
                            "checked" | "pressed" | "selected" | "expanded" if !pv.is_null() => line += &format!(" {pname}={}", pv.as_str().map(str::to_string).unwrap_or(pv.to_string())),
                            "level" if !pv.is_null() => line += &format!(" level={pv}"),
                            "url" if pv.is_string() => line += &format!(" url={:?}", trunc(pv.as_str().unwrap(), MAX_TEXT)),
                            _ => {}
                        }
                    }
                }
                out += &line;
                out.push('\n');
                lines += 1;
            } else {
                omitted += 1;
            }
            child_depth = depth + 1;
        }
        let pass_name = if fold { parent_name } else { name };
        let kids: Vec<&Value> = n["childIds"].as_array().into_iter().flatten().filter_map(|c| c.as_str().and_then(|id| by_id.get(id).copied())).collect();
        for k in kids.into_iter().rev() {
            stack.push((k, child_depth, pass_name.clone()));
        }
    }
    if omitted > 0 {
        out += &format!("… {omitted} more node(s) not shown; scroll, or browser_evaluate to read a part of the page\n");
    }
    (out, lines)
}

/// CDP key definitions for browser_press_key: (key, code, windowsVirtualKeyCode, text).
fn key_def(k: &str) -> Option<(String, String, u32, Option<String>)> {
    let named = |key: &str, code: &str, vk: u32, text: Option<&str>| Some((key.to_string(), code.to_string(), vk, text.map(str::to_string)));
    match k {
        "Enter" | "Return" => named("Enter", "Enter", 13, Some("\r")),
        "Tab" => named("Tab", "Tab", 9, None),
        "Escape" | "Esc" => named("Escape", "Escape", 27, None),
        "Backspace" => named("Backspace", "Backspace", 8, None),
        "Delete" => named("Delete", "Delete", 46, None),
        "Space" | " " => named(" ", "Space", 32, Some(" ")),
        "ArrowLeft" | "Left" => named("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowUp" | "Up" => named("ArrowUp", "ArrowUp", 38, None),
        "ArrowRight" | "Right" => named("ArrowRight", "ArrowRight", 39, None),
        "ArrowDown" | "Down" => named("ArrowDown", "ArrowDown", 40, None),
        "Home" => named("Home", "Home", 36, None),
        "End" => named("End", "End", 35, None),
        "PageUp" => named("PageUp", "PageUp", 33, None),
        "PageDown" => named("PageDown", "PageDown", 34, None),
        _ => {
            if let Some(n) = k.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n)) {
                return Some((k.to_string(), k.to_string(), 111 + n, None));
            }
            let mut cs = k.chars();
            let c = cs.next()?;
            if cs.next().is_some() {
                return None;
            }
            let (code, vk) = if c.is_ascii_alphabetic() {
                (format!("Key{}", c.to_ascii_uppercase()), c.to_ascii_uppercase() as u32)
            } else if c.is_ascii_digit() {
                (format!("Digit{c}"), c as u32)
            } else {
                (String::new(), 0)
            };
            Some((c.to_string(), code, vk, Some(c.to_string())))
        }
    }
}

fn modifier(m: &str) -> Option<(u32, &'static str, u32)> {
    match m.to_ascii_lowercase().as_str() {
        "alt" | "option" => Some((1, "Alt", 18)),
        "ctrl" | "control" => Some((2, "Control", 17)),
        "meta" | "cmd" | "command" | "super" => Some((4, "Meta", 91)),
        "shift" => Some((8, "Shift", 16)),
        "primary" => Some(if cfg!(target_os = "macos") { (4, "Meta", 91) } else { (2, "Control", 17) }),
        _ => None,
    }
}

const FILL_PREP: &str = "function(){const t=this.nodeType===1?this:this.parentElement;if(!t)return 'none';t.scrollIntoView({block:'center',inline:'nearest'});if(t.tagName==='SELECT')return 'select';t.focus();if(typeof t.select==='function'&&'value' in t){t.select();}else if(t.isContentEditable){const r=document.createRange();r.selectNodeContents(t);const s=getSelection();s.removeAllRanges();s.addRange(r);}let a=document.activeElement;while(a&&a.shadowRoot&&a.shadowRoot.activeElement)a=a.shadowRoot.activeElement;return (a&&(a===t||t.contains(a)||(t.shadowRoot&&t.shadowRoot.contains(a))))?'focused':'unfocused';}";
const FILL_SELECT: &str = "function(v){const o=[...this.options].find(o=>o.value===v)||[...this.options].find(o=>o.textContent.trim()===v);if(!o)return null;this.value=o.value;this.dispatchEvent(new Event('input',{bubbles:true}));this.dispatchEvent(new Event('change',{bubbles:true}));return o.textContent.trim();}";
const FILL_READ: &str = "function(){const t=this.nodeType===1?this:this.parentElement;return t&&('value' in t)?String(t.value):(t?t.innerText:'');}";

/// The page operations that change something and therefore need the task's lease.
pub const ACT_OPS: &[&str] = &["click", "hover", "fill", "type", "press_key", "scroll", "navigate", "new_page", "close_page", "evaluate"];

impl Broker {
    /// Run `f` on the browser connection, connecting first if needed. Reads retry once on a fresh
    /// connection when the old one died; acts never do, so nothing is clicked or typed twice.
    fn browser_cdp<T>(&self, f: impl Fn(&mut Cdp) -> Result<T>) -> Result<T> {
        self.browser_cdp_retry(true, f)
    }

    fn browser_cdp_retry<T>(&self, retry: bool, f: impl Fn(&mut Cdp) -> Result<T>) -> Result<T> {
        let mut g = self.browser.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..2 {
            if g.cdp.is_none() {
                g.cdp = Some(Cdp::connect(self.cfg.browser_cdp.as_deref())?);
            }
            match f(g.cdp.as_mut().unwrap()) {
                Err(e) if cdp::is_transport(&e) => {
                    g.cdp = None;
                    if attempt == 1 || !retry {
                        return Err(e);
                    }
                }
                r => return r,
            }
        }
        unreachable!()
    }

    /// Is a DevTools browser reachable right now? For `capabilities`, `doctor` and the CLI; needs no
    /// agent.
    pub fn browser_probe(&self) -> Value {
        match self.browser_cdp(|c| Ok((c.product.clone(), c.addr.clone(), c.source.clone(), c.pages()?.len()))) {
            Ok((product, addr, source, pages)) => json!({"available": true, "product": product, "endpoint": addr, "found_via": source, "pages": pages}),
            Err(e) => json!({"available": false, "error": e.message, "details": e.details}),
        }
    }

    pub(crate) fn browser_capability(&self) -> CapabilityReport {
        let p = self.browser_probe();
        let available = p["available"] == true;
        CapabilityReport {
            capability: Capability::Browser,
            availability: if available { Availability::Available } else { Availability::Unsupported },
            backend: "devtools".into(),
            reason: if available { format!("{} at {}", p["product"].as_str().unwrap_or(""), p["endpoint"].as_str().unwrap_or("")) } else { "no browser with a DevTools port; run `w3bs desktop browser launch`".into() },
            features: ["snapshot", "click", "fill", "type", "press_key", "scroll", "navigate", "new_page", "screenshot", "evaluate"].iter().map(|s| s.to_string()).collect(),
        }
    }

    fn browser_observe(&self, p: &Principal) -> Result<u64> {
        let gen0 = self.require_enabled()?;
        self.require_cap(p, Capability::Browser)?;
        Ok(gen0)
    }

    /// Everything an act needs before it touches the page.
    fn browser_act_gate(&self, p: &Principal, task: &str, op: &str, page: &str, args: &Value) -> Result<u64> {
        let gen0 = self.require_enabled()?;
        let pol = self.require_cap(p, Capability::Browser)?;
        self.checkpoint(p, task, gen0)?;
        let op_name = format!("browser_{op}");
        if pol.approval_required_for.contains("browser") || pol.approval_required_for.contains("input") || pol.approval_required_for.contains(&op_name) {
            let canonical = json!({"agent": p.agent, "task": task, "op": op_name, "page": page, "args": args});
            let summary = match op {
                "navigate" | "new_page" => format!("{} wants to open {} in the browser", p.agent, args["url"].as_str().unwrap_or("a page")),
                "fill" | "type" => format!("{} wants to type into the browser page", p.agent),
                _ => format!("{} wants to {} in the browser page", p.agent, op.replace('_', " ")),
            };
            self.require_approval(p, task, "browser", "browser", canonical, summary)?;
        }
        self.check_rate(p, &pol, 1)?;
        Ok(gen0)
    }

    fn browser_done(&self, p: &Principal, task: Option<&str>, op: &str, page: &str, r: &Result<Value>) {
        let outcome = match r {
            Ok(_) => "completed",
            Err(e) if e.code == ErrorCode::OutcomeUnknown => "outcome_unknown",
            Err(_) => "failed",
        };
        if let (Some(t), Ok(_)) = (task, r) {
            self.count_completed_batch(t);
        }
        self.audit.log(&format!("browser.{op}"), Some(&p.agent), task, "allowed", outcome, json!({"page": page, "error": r.as_ref().err().map(|e| e.code)}));
    }

    /// The page to act on: the one asked for, or the first ordinary tab.
    fn pick_page(c: &mut Cdp, page: Option<&str>) -> Result<String> {
        if let Some(p) = page.filter(|p| !p.is_empty()) {
            return Ok(p.to_string());
        }
        let pages = c.pages()?;
        pages
            .iter()
            .find(|t| t["url"].as_str().is_some_and(|u| u.starts_with("http")))
            .or(pages.first())
            .and_then(|t| t["targetId"].as_str().map(str::to_string))
            .ok_or_else(|| AdlError::not_found("the browser has no open page; use browser_new_page"))
    }

    fn page_info(c: &mut Cdp, page: &str) -> Value {
        c.page_call(page, "Runtime.evaluate", json!({"expression": "({url: location.href, title: document.title})", "returnByValue": true}))
            .map(|r| r["result"]["value"].clone())
            .unwrap_or(json!({}))
    }

    pub fn browser_status(&self, p: &Principal) -> Result<Value> {
        self.browser_observe(p)?;
        Ok(self.browser_probe())
    }

    pub fn browser_pages(&self, p: &Principal) -> Result<Value> {
        let gen0 = self.browser_observe(p)?;
        let pages = self.browser_cdp(|c| c.pages())?;
        self.still_valid(gen0)?;
        Ok(json!(pages.iter().map(|t| json!({"page": t["targetId"], "url": t["url"], "title": t["title"], "attached": t["attached"], "context": t["browserContextId"]})).collect::<Vec<_>>()))
    }

    pub fn browser_snapshot(&self, p: &Principal, page: Option<&str>, max_lines: Option<usize>) -> Result<Value> {
        let gen0 = self.browser_observe(p)?;
        let max = max_lines.unwrap_or(MAX_SNAPSHOT_LINES).clamp(50, 5000);
        let r = self.browser_cdp(|c| {
            let page = Self::pick_page(c, page)?;
            let info = Self::page_info(c, &page);
            let tree = c.page_call(&page, "Accessibility.getFullAXTree", json!({}))?;
            let nodes = tree["nodes"].as_array().cloned().unwrap_or_default();
            let (text, lines) = render_ax(&nodes, max);
            Ok(json!({"page": page, "url": info["url"], "title": info["title"], "lines": lines, "snapshot": text}))
        });
        self.still_valid(gen0)?;
        self.audit.log("browser.snapshot", Some(&p.agent), None, "allowed", if r.is_ok() { "completed" } else { "failed" }, json!({}));
        r
    }

    pub fn browser_screenshot(&self, p: &Principal, page: Option<&str>, full_page: bool) -> Result<Value> {
        let gen0 = self.browser_observe(p)?;
        let r = self.browser_cdp(|c| {
            let page = Self::pick_page(c, page)?;
            let mut params = json!({"format": "png"});
            if full_page {
                let m = c.page_call(&page, "Page.getLayoutMetrics", json!({}))?;
                let s = &m["cssContentSize"];
                let (w, h) = (s["width"].as_f64().unwrap_or(1280.0), s["height"].as_f64().unwrap_or(800.0).min(16_000.0));
                params = json!({"format": "png", "captureBeyondViewport": true, "clip": {"x": 0, "y": 0, "width": w, "height": h, "scale": 1}});
            }
            let shot = c.page_call(&page, "Page.captureScreenshot", params)?;
            Ok(json!({"page": page, "png_base64": shot["data"]}))
        });
        self.still_valid(gen0)?;
        self.audit.log("browser.screenshot", Some(&p.agent), None, "allowed", if r.is_ok() { "completed" } else { "failed" }, json!({}));
        r
    }

    /// Wait until the page's text contains `text`. Polls without holding the browser connection,
    /// so other calls are not blocked meanwhile.
    pub fn browser_wait_for(&self, p: &Principal, page: Option<&str>, text: &str, timeout_ms: Option<u64>) -> Result<Value> {
        let gen0 = self.browser_observe(p)?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(10_000).min(60_000));
        let page = self.browser_cdp(|c| Self::pick_page(c, page))?;
        let expr = format!("(document.body ? document.body.innerText : '').includes({})", json!(text));
        loop {
            self.still_valid(gen0)?;
            let found = self.browser_cdp(|c| c.page_call(&page, "Runtime.evaluate", json!({"expression": expr, "returnByValue": true}))).map(|r| r["result"]["value"] == true).unwrap_or(false);
            if found {
                return Ok(json!({"page": page, "found": true}));
            }
            if Instant::now() >= deadline {
                return Ok(json!({"page": page, "found": false}));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Run one page operation that changes something. `args` are the tool arguments.
    pub fn browser_act(&self, p: &Principal, task: &str, op: &str, page: Option<&str>, args: &Value) -> Result<Value> {
        if !ACT_OPS.contains(&op) {
            return Err(AdlError::invalid(format!("unknown browser operation {op}")));
        }
        if matches!(op, "navigate" | "new_page")
            && let Some(u) = args["url"].as_str()
            && !(u.starts_with("http://") || u.starts_with("https://") || u == "about:blank")
        {
            return Err(AdlError::denied("agents may open http, https and about:blank pages only"));
        }
        // new_page needs no existing page; every other op resolves one first so approval and
        // audit name it.
        let page_id = if op == "new_page" { String::new() } else { self.browser_cdp(|c| Self::pick_page(c, page))? };
        let gen0 = self.browser_act_gate(p, task, op, &page_id, args)?;
        let _dispatch = self.try_dispatch()?;
        let r = self.browser_cdp_retry(false, |c| {
            if op != "new_page" && op != "close_page" {
                let _ = c.page_call(&page_id, "Page.bringToFront", json!({}));
            }
            self.checkpoint(p, task, gen0)?;
            match op {
                "click" | "hover" => {
                    let uid = args["uid"].as_u64().ok_or_else(|| AdlError::invalid("uid is required (from browser_snapshot)"))?;
                    let (x, y) = Self::node_center(c, &page_id, uid)?;
                    c.page_call(&page_id, "Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": x, "y": y}))?;
                    if op == "click" {
                        let count = args["count"].as_u64().unwrap_or(1).clamp(1, 3);
                        let button = args["button"].as_str().unwrap_or("left");
                        for n in 1..=count {
                            c.page_call(&page_id, "Input.dispatchMouseEvent", json!({"type": "mousePressed", "x": x, "y": y, "button": button, "buttons": 1, "clickCount": n}))?;
                            c.page_call(&page_id, "Input.dispatchMouseEvent", json!({"type": "mouseReleased", "x": x, "y": y, "button": button, "buttons": 0, "clickCount": n}))?;
                        }
                    }
                    Ok(json!({"page": page_id, "x": x, "y": y}))
                }
                "fill" => {
                    let uid = args["uid"].as_u64().ok_or_else(|| AdlError::invalid("uid is required (from browser_snapshot)"))?;
                    let value = args["value"].as_str().ok_or_else(|| AdlError::invalid("value is required"))?;
                    let obj = Self::resolve_uid(c, &page_id, uid)?;
                    let how = c.page_call(&page_id, "Runtime.callFunctionOn", json!({"objectId": obj, "functionDeclaration": FILL_PREP, "returnByValue": true}))?["result"]["value"].as_str().unwrap_or("").to_string();
                    match how.as_str() {
                        "select" => {
                            let chosen = c.page_call(&page_id, "Runtime.callFunctionOn", json!({"objectId": obj, "functionDeclaration": FILL_SELECT, "arguments": [{"value": value}], "returnByValue": true}))?;
                            if chosen["result"]["value"].is_null() {
                                return Err(AdlError::not_found("no option with that value or label"));
                            }
                            Ok(json!({"page": page_id, "selected": chosen["result"]["value"]}))
                        }
                        "focused" => {
                            if value.is_empty() {
                                Self::key(c, &page_id, "Backspace", &[])?;
                            } else {
                                c.page_call(&page_id, "Input.insertText", json!({"text": value}))?;
                            }
                            let now = c.page_call(&page_id, "Runtime.callFunctionOn", json!({"objectId": obj, "functionDeclaration": FILL_READ, "returnByValue": true}))?;
                            Ok(json!({"page": page_id, "verified": now["result"]["value"].as_str() == Some(value)}))
                        }
                        _ => Err(AdlError::invalid("that element cannot take text (not an input, textarea, select or editable element); click it or pick another uid")),
                    }
                }
                "type" => {
                    let text = args["text"].as_str().ok_or_else(|| AdlError::invalid("text is required"))?;
                    c.page_call(&page_id, "Input.insertText", json!({"text": text}))?;
                    if args["submit"] == true {
                        Self::key(c, &page_id, "Enter", &[])?;
                    }
                    Ok(json!({"page": page_id}))
                }
                "press_key" => {
                    let combo = args["key"].as_str().ok_or_else(|| AdlError::invalid("key is required, e.g. Enter, Tab, Control+A"))?;
                    let parts: Vec<&str> = combo.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
                    let (key, mods) = parts.split_last().ok_or_else(|| AdlError::invalid("key is required"))?;
                    Self::key(c, &page_id, key, mods)?;
                    Ok(json!({"page": page_id}))
                }
                "scroll" => {
                    let dy = args["dy"].as_f64().unwrap_or(600.0);
                    let dx = args["dx"].as_f64().unwrap_or(0.0);
                    let (x, y) = match args["uid"].as_u64() {
                        Some(uid) => Self::node_center(c, &page_id, uid)?,
                        None => {
                            let m = c.page_call(&page_id, "Page.getLayoutMetrics", json!({}))?;
                            let v = &m["cssVisualViewport"];
                            (v["clientWidth"].as_f64().unwrap_or(800.0) / 2.0, v["clientHeight"].as_f64().unwrap_or(600.0) / 2.0)
                        }
                    };
                    c.page_call(&page_id, "Input.dispatchMouseEvent", json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy}))?;
                    Ok(json!({"page": page_id}))
                }
                "navigate" => {
                    let timeout = Duration::from_millis(args["timeout_ms"].as_u64().unwrap_or(15_000).min(60_000));
                    // Mark the current document, so the wait below cannot mistake it (still
                    // "complete") for the new one. A same-document move changes only the URL.
                    let token = crate::util::random_id("nav", 8);
                    let before = c
                        .page_call(&page_id, "Runtime.evaluate", json!({"expression": format!("window.__w3bsNav = {}; location.href", json!(token)), "returnByValue": true}))?["result"]["value"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let done = format!("(window.__w3bsNav !== {} || location.href !== {}) && document.readyState === 'complete'", json!(token), json!(before));
                    match (args["url"].as_str(), args["action"].as_str()) {
                        (Some(u), _) => {
                            let r = c.page_call(&page_id, "Page.navigate", json!({"url": u}))?;
                            if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
                                return Err(AdlError::new(ErrorCode::Invalid, format!("navigation failed: {e}")));
                            }
                        }
                        (None, Some("back")) => drop(c.page_call(&page_id, "Runtime.evaluate", json!({"expression": "history.back()"}))?),
                        (None, Some("forward")) => drop(c.page_call(&page_id, "Runtime.evaluate", json!({"expression": "history.forward()"}))?),
                        (None, Some("reload")) => drop(c.page_call(&page_id, "Page.reload", json!({}))?),
                        _ => return Err(AdlError::invalid("give url, or action back|forward|reload")),
                    }
                    let loaded = Self::wait_loaded(c, &page_id, timeout, &done);
                    let mut info = Self::page_info(c, &page_id);
                    info["page"] = json!(page_id);
                    info["loaded"] = json!(loaded);
                    Ok(info)
                }
                "new_page" => {
                    let url = args["url"].as_str().unwrap_or("about:blank");
                    let mut params = json!({"url": url});
                    // Open next to an existing page in the same browser context (an incognito
                    // window's tab stays incognito).
                    if let Some(like) = args["like_page"].as_str()
                        && let Some(t) = c.pages()?.into_iter().find(|t| t["targetId"] == like)
                        && let Some(ctx) = t["browserContextId"].as_str()
                    {
                        params["browserContextId"] = json!(ctx);
                    }
                    let r = c.call(None, "Target.createTarget", params)?;
                    let id = r["targetId"].as_str().unwrap_or_default().to_string();
                    // A new tab starts on about:blank before it commits the url.
                    let done = if url == "about:blank" { "document.readyState === 'complete'".to_string() } else { "location.href !== 'about:blank' && document.readyState === 'complete'".to_string() };
                    let loaded = Self::wait_loaded(c, &id, Duration::from_secs(15), &done);
                    let mut info = Self::page_info(c, &id);
                    info["page"] = json!(id);
                    info["loaded"] = json!(loaded);
                    Ok(info)
                }
                "close_page" => {
                    c.call(None, "Target.closeTarget", json!({"targetId": page_id}))?;
                    c.forget(&page_id);
                    Ok(json!({"page": page_id, "closed": true}))
                }
                "evaluate" => {
                    let expr = args["expression"].as_str().ok_or_else(|| AdlError::invalid("expression is required"))?;
                    let r = c.page_call(&page_id, "Runtime.evaluate", json!({"expression": expr, "returnByValue": true, "awaitPromise": true, "userGesture": true}))?;
                    if let Some(ex) = r.get("exceptionDetails") {
                        let msg = ex["exception"]["description"].as_str().or(ex["text"].as_str()).unwrap_or("exception");
                        return Err(AdlError::invalid(format!("the script threw: {}", trunc(msg, 500))));
                    }
                    let v = r["result"]["value"].clone();
                    let s = v.to_string();
                    Ok(if s.len() > MAX_EVAL_BYTES { json!({"page": page_id, "truncated": true, "result": s.chars().take(MAX_EVAL_BYTES).collect::<String>()}) } else { json!({"page": page_id, "result": v}) })
                }
                _ => unreachable!(),
            }
        });
        let r = r.and_then(|v| {
            self.still_valid(gen0)?;
            Ok(v)
        });
        self.browser_done(p, Some(task), op, &page_id, &r);
        r
    }

    /// Resolve a uid in the page's *current* document. Resolving it in a fresh isolated world of
    /// the live main frame fails for a node of any other document, including the previous page
    /// that the back/forward cache keeps alive with its layout (whose boxes would otherwise send a
    /// click to the same coordinates of the new page).
    fn resolve_uid(c: &mut Cdp, page: &str, uid: u64) -> Result<String> {
        let stale = || AdlError::stale(format!("uid {uid} is not on the page any more; take a new browser_snapshot"));
        let keep = |e: AdlError| if cdp::is_transport(&e) { e } else { stale() };
        let tree = c.page_call(page, "Page.getFrameTree", json!({}))?;
        let frame = tree["frameTree"]["frame"]["id"].as_str().unwrap_or(page).to_string();
        let world = c.page_call(page, "Page.createIsolatedWorld", json!({"frameId": frame, "worldName": "w3bs-desktop"}))?;
        let r = c.page_call(page, "DOM.resolveNode", json!({"backendNodeId": uid, "executionContextId": world["executionContextId"]})).map_err(keep)?;
        r["object"]["objectId"].as_str().map(str::to_string).ok_or_else(stale)
    }

    /// Scroll a node into view and return the centre of its first visible box, in CSS pixels.
    fn node_center(c: &mut Cdp, page: &str, uid: u64) -> Result<(f64, f64)> {
        let stale = || AdlError::stale(format!("uid {uid} is not on the page any more; take a new browser_snapshot"));
        Self::resolve_uid(c, page, uid)?;
        let _ = c.page_call(page, "DOM.scrollIntoViewIfNeeded", json!({"backendNodeId": uid}));
        let quads = c.page_call(page, "DOM.getContentQuads", json!({"backendNodeId": uid})).map_err(|e| if cdp::is_transport(&e) { e } else { stale() })?;
        for q in quads["quads"].as_array().into_iter().flatten() {
            let v: Vec<f64> = q.as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
            if v.len() == 8 {
                let (xs, ys) = ([v[0], v[2], v[4], v[6]], [v[1], v[3], v[5], v[7]]);
                let w = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
                let h = ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min);
                if w >= 1.0 && h >= 1.0 {
                    return Ok((xs.iter().sum::<f64>() / 4.0, ys.iter().sum::<f64>() / 4.0));
                }
            }
        }
        Err(AdlError::invalid(format!("uid {uid} has no visible box (hidden or zero-size); pick another uid")))
    }

    fn key(c: &mut Cdp, page: &str, key: &str, mods: &[&str]) -> Result<()> {
        let (k, code, vk, text) = key_def(key).ok_or_else(|| AdlError::invalid(format!("unknown key {key}")))?;
        let mut bits = 0;
        let mut held = vec![];
        for m in mods {
            let (bit, name, mvk) = modifier(m).ok_or_else(|| AdlError::invalid(format!("unknown modifier {m}")))?;
            bits |= bit;
            c.page_call(page, "Input.dispatchKeyEvent", json!({"type": "rawKeyDown", "key": name, "code": format!("{name}Left"), "windowsVirtualKeyCode": mvk, "modifiers": bits}))?;
            held.push((name, mvk));
        }
        // With Control/Alt/Meta held the key is a shortcut, not text.
        let text = text.filter(|_| bits & 7 == 0);
        let down = match &text {
            Some(t) => json!({"type": "keyDown", "key": k, "code": code, "windowsVirtualKeyCode": vk, "text": t, "unmodifiedText": t, "modifiers": bits}),
            None => json!({"type": "rawKeyDown", "key": k, "code": code, "windowsVirtualKeyCode": vk, "modifiers": bits}),
        };
        c.page_call(page, "Input.dispatchKeyEvent", down)?;
        c.page_call(page, "Input.dispatchKeyEvent", json!({"type": "keyUp", "key": k, "code": code, "windowsVirtualKeyCode": vk, "modifiers": bits}))?;
        for (name, mvk) in held.into_iter().rev() {
            c.page_call(page, "Input.dispatchKeyEvent", json!({"type": "keyUp", "key": name, "code": format!("{name}Left"), "windowsVirtualKeyCode": mvk, "modifiers": 0}))?;
        }
        Ok(())
    }

    /// Poll `done` (a JS boolean expression) until it holds; errors while the document is being
    /// replaced count as not yet.
    fn wait_loaded(c: &mut Cdp, page: &str, timeout: Duration, done: &str) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(r) = c.page_call_timeout(page, "Runtime.evaluate", json!({"expression": done, "returnByValue": true}), Duration::from_secs(5))
                && r["result"]["value"] == true
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_outline_with_uids_and_folds_generic() {
        let nodes = json!([
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Login"}, "childIds": ["2"], "backendDOMNodeId": 1},
            {"nodeId": "2", "parentId": "1", "role": {"value": "generic"}, "name": {"value": ""}, "childIds": ["3", "4", "6"]},
            {"nodeId": "3", "parentId": "2", "role": {"value": "textbox"}, "name": {"value": "Username"}, "value": {"value": "rc"}, "properties": [{"name": "focused", "value": {"value": true}}], "childIds": [], "backendDOMNodeId": 31},
            {"nodeId": "4", "parentId": "2", "role": {"value": "button"}, "name": {"value": "Sign in"}, "childIds": ["5"], "backendDOMNodeId": 42},
            {"nodeId": "5", "parentId": "4", "role": {"value": "StaticText"}, "name": {"value": "Sign in"}, "childIds": []},
            {"nodeId": "6", "parentId": "2", "ignored": true, "role": {"value": "none"}, "childIds": []}
        ]);
        let (text, lines) = render_ax(nodes.as_array().unwrap(), 100);
        assert_eq!(lines, 3, "{text}");
        assert!(text.contains("uid=1 RootWebArea \"Login\""));
        assert!(text.contains("  uid=31 textbox \"Username\" value=\"rc\" focused"));
        assert!(text.contains("  uid=42 button \"Sign in\""));
        assert!(!text.contains("text \"Sign in\""), "a label repeating its parent's name is folded");
    }

    #[test]
    fn caps_lines() {
        let mut nodes = vec![json!({"nodeId": "0", "role": {"value": "RootWebArea"}, "name": {"value": "p"}, "childIds": (1..=20).map(|i| i.to_string()).collect::<Vec<_>>()})];
        for i in 1..=20 {
            nodes.push(json!({"nodeId": i.to_string(), "parentId": "0", "role": {"value": "link"}, "name": {"value": format!("l{i}")}, "childIds": [], "backendDOMNodeId": i}));
        }
        let (text, lines) = render_ax(&nodes, 5);
        assert_eq!(lines, 5);
        assert!(text.contains("16 more node(s)"));
    }

    #[test]
    fn keys() {
        assert_eq!(key_def("Enter").unwrap().2, 13);
        assert_eq!(key_def("a").unwrap().1, "KeyA");
        assert_eq!(key_def("F5").unwrap().2, 116);
        assert!(key_def("Nope").is_none());
        assert_eq!(modifier("ctrl").unwrap().0, 2);
    }
}
