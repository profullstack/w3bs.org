//! `w3bs ask <agent> "question"`: ask a published agent, answered on the c0mpute network.
//!
//! The agents are a small JSON registry (w3bs.org/agents.json, a copy built in). Each names
//! a model Infernet nodes serve (a house model every node pulls), a system prompt and its
//! grounding. Small models supply the voice, not the facts, so the question first goes to
//! pit.world search and the top results go into the prompt; the CLI prints them as sources.
//!
//! Transport: Infernet's public chat API (`POST /api/chat` with `p2p_only`, then the SSE
//! stream), which routes the job to a live c0mpute/Infernet node and never to a hosted
//! (paid) fallback. Anonymous use shares Infernet's public limit (20 questions an hour per
//! IP); `INFERNET_API_KEY` sends a trusted client key instead. Nothing runs locally.

use futures::StreamExt;
use serde_json::{Value, json};
use std::io::Write;
use std::time::{Duration, Instant};
use w3bs_core::{Error, Result};

/// The registry this binary was built with; used when w3bs.org cannot be reached.
pub const BUILTIN: &str = include_str!("../../../../fixtures/agents.json");
const REGISTRY: &str = "https://w3bs.org/agents.json";
const INFERNET: &str = "https://infernetprotocol.com";
const SEARCH: &str = "https://pit.world/api/search";
const SNIPPET_CHARS: usize = 700;

#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub name: String,
    pub aliases: Vec<String>,
    pub title: String,
    pub by: String,
    pub model: String,
    pub huggingface: String,
    pub gguf: String,
    pub system: String,
    /// Shown under every answer (legal and tax agents: "not a lawyer, not advice").
    pub disclaimer: String,
    pub search: bool,
    pub query_suffix: String,
    pub results: usize,
    pub max_tokens: u64,
    pub temperature: f64,
}

impl Agent {
    fn from_value(v: &Value) -> Option<Agent> {
        let st = |k: &str| v[k].as_str().unwrap_or_default().trim().to_owned();
        let name = st("name").to_lowercase();
        if name.is_empty() || name.len() > 40 || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return None;
        }
        let (model, system) = (st("model"), st("system"));
        if model.is_empty() || system.is_empty() {
            return None;
        }
        let g = &v["grounding"];
        Some(Agent {
            aliases: v["aliases"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect()).unwrap_or_default(),
            title: st("title"),
            by: st("by"),
            huggingface: st("huggingface"),
            gguf: st("gguf"),
            disclaimer: st("disclaimer"),
            search: g["search"].as_str().is_some_and(|s| !s.is_empty()),
            query_suffix: g["query_suffix"].as_str().unwrap_or_default().trim().to_owned(),
            results: g["results"].as_u64().unwrap_or(3).clamp(1, 8) as usize,
            max_tokens: v["max_tokens"].as_u64().unwrap_or(350).clamp(16, 2048),
            temperature: v["temperature"].as_f64().unwrap_or(0.2).clamp(0.0, 2.0),
            name,
            model,
            system,
        })
    }

    pub fn summary(&self) -> Value {
        let mut v = json!({ "name": self.name, "aliases": self.aliases, "title": self.title, "by": self.by, "model": self.model,
                "huggingface": self.huggingface, "gguf": self.gguf, "grounding": if self.search { "pit.world" } else { "none" } });
        if !self.disclaimer.is_empty() {
            v["disclaimer"] = json!(self.disclaimer);
        }
        v
    }
}

/// Parse a registry document; agents that are malformed are skipped, not fatal.
pub fn parse_registry(text: &str) -> Result<Vec<Agent>> {
    let v: Value = serde_json::from_str(text).map_err(|e| Error::package("ASK_BAD_REGISTRY", format!("the agent registry is not JSON: {e}")))?;
    let agents: Vec<Agent> = v["agents"].as_array().map(|a| a.iter().filter_map(Agent::from_value).collect()).unwrap_or_default();
    if agents.is_empty() {
        return Err(Error::package("ASK_BAD_REGISTRY", "the agent registry lists no usable agents"));
    }
    Ok(agents)
}

/// An agent by name or alias, case-insensitively.
pub fn find<'a>(agents: &'a [Agent], name: &str) -> Option<&'a Agent> {
    let n = name.trim().trim_start_matches('@').to_lowercase();
    agents.iter().find(|a| a.name == n).or_else(|| agents.iter().find(|a| a.aliases.contains(&n)))
}

fn names(agents: &[Agent]) -> String {
    agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
}

pub fn unknown(agents: &[Agent], name: &str) -> Error {
    Error::usage("ASK_UNKNOWN_AGENT", format!("no agent called {name:?}; agents: {} (see `w3bs ask --list`)", names(agents)))
}

/// HTTPS, or plain HTTP to loopback for development and tests.
fn checked_url(raw: &str, what: &str) -> Result<url::Url> {
    let u = url::Url::parse(raw).map_err(|e| Error::usage("ASK_BAD_URL", format!("{what} {raw:?}: {e}")))?;
    let loopback = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if u.scheme() != "https" && !(u.scheme() == "http" && loopback) {
        return Err(Error::usage("HTTPS_REQUIRED", format!("{what} must use HTTPS (plain HTTP only to loopback)")));
    }
    Ok(u)
}

fn http(timeout: Option<Duration>) -> reqwest::Client {
    let mut b = reqwest::Client::builder().user_agent(format!("w3bs/{} (ask)", w3bs_core::engine::MANAGER_VERSION)).connect_timeout(Duration::from_secs(10));
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    b.build().expect("http client")
}

pub struct Loaded {
    pub agents: Vec<Agent>,
    pub source: String,
    pub warning: Option<String>,
}

/// `$W3BS_AGENTS` or w3bs.org/agents.json; the built-in copy when that fails.
pub async fn load() -> Loaded {
    let url = std::env::var("W3BS_AGENTS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| REGISTRY.into());
    let fetched = async {
        let u = checked_url(&url, "W3BS_AGENTS")?;
        let resp = http(Some(Duration::from_secs(5))).get(u).send().await.map_err(|e| Error::package("ASK_REGISTRY_UNAVAILABLE", e.to_string()))?;
        if !resp.status().is_success() {
            return Err(Error::package("ASK_REGISTRY_UNAVAILABLE", format!("HTTP {}", resp.status().as_u16())));
        }
        parse_registry(&resp.text().await.unwrap_or_default())
    }
    .await;
    match fetched {
        Ok(agents) => Loaded { agents, source: url, warning: None },
        Err(e) => Loaded {
            agents: parse_registry(BUILTIN).expect("the built-in agent registry parses"),
            source: "built-in".into(),
            warning: Some(format!("could not read {url} ({}); using the agents built into this w3bs", e.info().message)),
        },
    }
}

// ------------------------------------------------------------------ grounding

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub title: String,
    pub url: String,
    pub text: String,
}

/// The search query: the question with runs of punctuation calmed down, plus the agent's suffix.
pub fn search_query(question: &str, suffix: &str) -> String {
    let mut q = String::new();
    let mut last = ' ';
    for c in question.chars() {
        if matches!(c, '?' | '!' | '.') && c == last {
            continue;
        }
        q.push(c);
        last = c;
    }
    let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
    if suffix.is_empty() { q } else { format!("{q} {suffix}") }
}

/// The top `k` results with a title and an http(s) URL; results with a description first,
/// in the engine's order, because an empty snippet gives the model nothing to cite.
pub fn pick_sources(body: &Value, k: usize) -> Vec<Source> {
    let all: Vec<Source> = body["results"]
        .as_array()
        .map(|r| {
            r.iter()
                .filter_map(|x| {
                    let (title, url) = (x["title"].as_str()?.trim(), x["url"].as_str()?.trim());
                    if title.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
                        return None;
                    }
                    let text: String = x["description"].as_str().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(SNIPPET_CHARS).collect();
                    Some(Source { title: title.to_owned(), url: url.to_owned(), text })
                })
                .collect()
        })
        .unwrap_or_default();
    let (with, without): (Vec<_>, Vec<_>) = all.into_iter().partition(|s| !s.text.is_empty());
    with.into_iter().chain(without).take(k).collect()
}

async fn search(question: &str, agent: &Agent) -> std::result::Result<Vec<Source>, String> {
    let raw = std::env::var("W3BS_SEARCH_URL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| SEARCH.into());
    let mut u = checked_url(&raw, "W3BS_SEARCH_URL").map_err(|e| e.info().message)?;
    u.query_pairs_mut().append_pair("q", &search_query(question, &agent.query_suffix));
    let resp = http(Some(Duration::from_secs(20))).get(u).header("accept", "application/json").send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(pick_sources(&body, agent.results))
}

/// The answer without a leading copy of the system prompt (small models sometimes echo it).
pub fn strip_echo(answer: &str, system: &str) -> String {
    let a = answer.trim();
    a.strip_prefix(system.trim()).map(str::trim).unwrap_or(a).to_owned()
}

/// The chat messages: the agent's system prompt, then the question with numbered background.
///
/// Layout 0 puts the question first in the user turn (with the background first, a 0.5B model
/// tends to repeat its system prompt). Layout 1, the retry after such an echo, moves the
/// background into the system message and leaves the user turn as just the question.
pub fn messages(agent: &Agent, question: &str, sources: &[Source], layout: u8) -> Value {
    if sources.is_empty() {
        return json!([{ "role": "system", "content": agent.system }, { "role": "user", "content": question }]);
    }
    let ctx = sources.iter().enumerate().map(|(i, s)| format!("[{}] {} ({})\n{}", i + 1, s.title, s.url, s.text)).collect::<Vec<_>>().join("\n\n");
    if layout == 0 {
        let user = format!("Question: {question}\n\nNotes from the web:\n{ctx}\n\nAnswer the question in a few practical steps.");
        json!([{ "role": "system", "content": agent.system }, { "role": "user", "content": user }])
    } else {
        let system = format!("{}\n\nNotes from the web you may use:\n{ctx}", agent.system);
        json!([{ "role": "system", "content": system }, { "role": "user", "content": question }])
    }
}

/// Holds streamed tokens back while they could still be a copy of the system prompt, so an
/// echo never reaches the screen; everything after the echo (or a real answer) flows through.
#[derive(Debug, Default)]
pub struct EchoGuard {
    held: String,
    released: bool,
}

impl EchoGuard {
    /// Text to show now, if any.
    pub fn push(&mut self, t: &str, system: &str) -> Option<String> {
        if self.released {
            return Some(t.to_owned());
        }
        self.held.push_str(t);
        let h = self.held.trim_start();
        let sys = system.trim();
        if sys.starts_with(h) {
            return None;
        }
        if let Some(rest) = h.strip_prefix(sys) {
            let rest = rest.trim_start();
            if rest.is_empty() {
                return None;
            }
            self.released = true;
            return Some(rest.to_owned());
        }
        self.released = true;
        Some(std::mem::take(&mut self.held))
    }
}

// ------------------------------------------------------------------ Infernet SSE

#[derive(Debug, Default)]
pub struct Sse {
    buf: String,
}

impl Sse {
    /// Feed bytes; returns every complete `(event, data)` frame.
    pub fn push(&mut self, chunk: &str) -> Vec<(String, Value)> {
        self.buf.push_str(&chunk.replace("\r\n", "\n"));
        let mut out = vec![];
        while let Some(end) = self.buf.find("\n\n") {
            let frame: String = self.buf.drain(..end + 2).collect();
            let (mut event, mut data) = (String::from("message"), String::new());
            for line in frame.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim().to_owned();
                } else if let Some(v) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(v.strip_prefix(' ').unwrap_or(v));
                }
            }
            if !data.is_empty() {
                out.push((event, serde_json::from_str(&data).unwrap_or(Value::String(data))));
            }
        }
        out
    }
}

pub struct Request<'a> {
    pub agent: &'a Agent,
    pub question: &'a str,
    pub search: bool,
    pub max_tokens: Option<u64>,
    pub infernet: Option<&'a str>,
    pub timeout: Duration,
}

/// Ask one question. `on_token` sees the answer as it streams; `on_status` sees progress.
pub async fn ask(req: Request<'_>, on_token: &mut dyn FnMut(&str), on_status: &mut dyn FnMut(&str)) -> Result<Value> {
    let agent = req.agent;
    let question = req.question.trim();
    if question.is_empty() {
        return Err(Error::usage("ASK_NO_QUESTION", format!("ask a question: w3bs ask {} \"How should I price my first plan?\"", agent.name)));
    }
    let started = Instant::now();
    let mut warnings = vec![];
    let sources = if req.search && agent.search {
        on_status("searching pit.world");
        match search(question, agent).await {
            Ok(s) => s,
            Err(e) => {
                warnings.push(format!("pit.world search failed ({e}); answering without sources"));
                vec![]
            }
        }
    } else {
        vec![]
    };

    let origin_raw = req.infernet.map(str::to_owned).or_else(|| std::env::var("W3BS_INFERNET_API").ok().filter(|s| !s.is_empty())).unwrap_or_else(|| INFERNET.into());
    let origin = checked_url(&origin_raw, "the Infernet API")?.origin().ascii_serialization();
    let key = std::env::var("INFERNET_API_KEY").ok().filter(|s| !s.trim().is_empty());
    let (mut node, mut job, mut final_text) = (Value::Null, String::new(), String::new());
    let mut first_token = None::<u128>;
    for attempt in 0..2u8 {
        let body = json!({
            "modelName": agent.model,
            "messages": messages(agent, question, &sources, attempt),
            "maxTokens": req.max_tokens.unwrap_or(agent.max_tokens),
            "temperature": agent.temperature,
            // Only a live peer node may answer: never Infernet's hosted (paid) fallback.
            "p2p_only": true,
        });
        on_status(&format!("asking {} on the c0mpute network", agent.name));
        let client = http(Some(Duration::from_secs(30)));
        let mut post = client.post(format!("{origin}/api/chat")).json(&body);
        if let Some(k) = &key {
            post = post.bearer_auth(k.trim());
        }
        let resp = post.send().await.map_err(|e| Error::package("ASK_UNAVAILABLE", format!("{origin}/api/chat: {e}")).retryable())?;
        let status = resp.status().as_u16();
        let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default()).unwrap_or(Value::Null);
        let msg = v["error"].as_str().map(str::to_owned).unwrap_or_else(|| format!("the Infernet API answered {status}"));
        match status {
            200..=299 => {}
            429 => {
                let at = v["detail"]["resetAt"].as_str().map(|t| format!(" (resets {t})")).unwrap_or_default();
                let hint = if key.is_some() { "your INFERNET_API_KEY budget is spent".to_owned() } else { "the public limit is 20 questions an hour per IP; set INFERNET_API_KEY to a trusted client key for more".to_owned() };
                return Err(Error::package("ASK_RATE_LIMITED", format!("Infernet rate limit reached{at}: {hint}")).retryable());
            }
            503 => {
                let live = v["detail"]["availableModels"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                return Err(Error::package("ASK_NO_NODE", format!("no c0mpute node is serving {} right now; try again in a few minutes{}", agent.model, if live.is_empty() { String::new() } else { format!(" (live models: {live})") })).retryable());
            }
            401 | 403 => return Err(Error::package("ASK_REFUSED", format!("Infernet refused the question: {msg}"))),
            _ => return Err(Error::package("ASK_FAILED", format!("Infernet: {msg}")).retryable()),
        }
        if v["source"].as_str() != Some("p2p") {
            return Err(Error::package("ASK_NOT_P2P", format!("Infernet offered a {:?} route, not a c0mpute node; refused", v["source"].as_str().unwrap_or("unknown"))));
        }
        job = v["jobId"].as_str().unwrap_or_default().to_owned();
        let stream_path = v["streamUrl"].as_str().unwrap_or_default();
        if job.is_empty() || !stream_path.starts_with("/api/chat/stream/") || stream_path.contains("..") {
            return Err(Error::package("ASK_FAILED", "Infernet's answer had no job stream"));
        }
        node = json!({ "id": v["provider"]["nodeId"], "name": v["provider"]["name"], "provider_id": v["provider"]["id"], "gpu": v["provider"]["gpuModel"] });
        on_status(&format!("{} is answering", s(&node["name"])));

        let mut get = http(None).get(format!("{origin}{stream_path}")).header("accept", "text/event-stream");
        if let Some(k) = &key {
            get = get.bearer_auth(k.trim());
        }
        let deadline = started + req.timeout;
        let resp = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), get.send())
            .await
            .map_err(|_| Error::package("ASK_TIMEOUT", "the node did not start answering in time").retryable())?
            .map_err(|e| Error::package("ASK_UNAVAILABLE", format!("{origin}{stream_path}: {e}")).retryable())?;
        if !resp.status().is_success() {
            return Err(Error::package("ASK_FAILED", format!("the answer stream returned HTTP {}", resp.status().as_u16())).retryable());
        }
        let mut stream = resp.bytes_stream();
        let mut sse = Sse::default();
        let (mut answer, mut finished, mut guard) = (String::new(), None::<String>, EchoGuard::default());
        'read: loop {
            let chunk = match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), stream.next()).await {
                Err(_) => return Err(Error::package("ASK_TIMEOUT", format!("no complete answer after {} s (job {job})", req.timeout.as_secs())).retryable()),
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(Error::package("ASK_UNAVAILABLE", format!("the answer stream broke: {e}")).retryable()),
                Ok(Some(Ok(b))) => b,
            };
            for (event, data) in sse.push(&String::from_utf8_lossy(&chunk)) {
                match event.as_str() {
                    "meta" => {
                        for (k, from) in [("id", "provider_node_id"), ("name", "provider_name"), ("engine", "engine")] {
                            if let Some(x) = data[from].as_str() {
                                node[k] = json!(x);
                            }
                        }
                    }
                    "token" => {
                        if let Some(t) = data["text"].as_str() {
                            answer.push_str(t);
                            if let Some(show) = guard.push(t, &agent.system) {
                                first_token.get_or_insert(started.elapsed().as_millis());
                                on_token(&show);
                            }
                        }
                    }
                    "done" => {
                        finished = Some(data["text"].as_str().map(str::to_owned).unwrap_or_else(|| answer.clone()));
                        break 'read;
                    }
                    "error" => {
                        let m = data["message"].as_str().or(data["error"].as_str()).or(data.as_str()).unwrap_or("the node failed").to_owned();
                        return Err(Error::package("ASK_FAILED", format!("{} could not answer: {m}", s(&node["name"]))).retryable());
                    }
                    _ => {}
                }
            }
        }
        let Some(text) = finished else {
            return Err(Error::package("ASK_FAILED", format!("the answer stream ended before the node finished (job {job})")).retryable());
        };
        let text = strip_echo(&text, &agent.system);
        if text.is_empty() {
            if attempt == 0 {
                on_status(&format!("{} repeated its instructions; asking again", s(&node["name"])));
                continue;
            }
            return Err(Error::package("ASK_NO_ANSWER", format!("{} repeated its instructions instead of answering, twice (job {job}); ask again, perhaps in other words", agent.name)).retryable());
        }
        if !guard.released {
            on_token(&text);
        }
        final_text = text;
        break;
    }
    let mut out = json!({
        "agent": agent.name,
        "title": agent.title,
        "model": agent.model,
        "question": question,
        "answer": final_text,
        "sources": sources.iter().enumerate().map(|(i, s)| json!({ "n": i + 1, "title": s.title, "url": s.url })).collect::<Vec<_>>(),
        "node": node,
        "transport": "infernet",
        "infernet": origin,
        "job_id": job,
        "authenticated": key.is_some(),
        "latency_ms": started.elapsed().as_millis() as u64,
        "first_token_ms": first_token.map(|t| t as u64),
        "warnings": warnings,
    });
    if !agent.disclaimer.is_empty() {
        out["disclaimer"] = json!(agent.disclaimer);
    }
    Ok(out)
}

fn s(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| "a node".into())
}

/// What follows the streamed answer: the sources, then who served it (stderr).
pub fn print_footer(d: &Value) {
    let sources = d["sources"].as_array().cloned().unwrap_or_default();
    if !sources.is_empty() {
        println!("\nSources:");
        for x in &sources {
            println!("  [{}] {}\n      {}", x["n"], x["title"].as_str().unwrap_or_default(), x["url"].as_str().unwrap_or_default());
        }
    }
    if let Some(note) = d["disclaimer"].as_str().filter(|n| !n.is_empty()) {
        println!("\n{note}");
    }
    let secs = |k: &str| d[k].as_u64().map(|ms| format!("{:.1} s", ms as f64 / 1000.0));
    let first = secs("first_token_ms").map(|t| format!(", first token {t}")).unwrap_or_default();
    eprintln!(
        "\n{} answered with {} on c0mpute node {} ({}) in {}{first}",
        d["agent"].as_str().unwrap_or_default(),
        d["model"].as_str().unwrap_or_default(),
        s(&d["node"]["name"]),
        d["node"]["id"].as_str().unwrap_or("?"),
        secs("latency_ms").unwrap_or_default(),
    );
}

pub fn print_list(d: &Value) {
    let agents = d["agents"].as_array().cloned().unwrap_or_default();
    for a in &agents {
        let aliases = a["aliases"].as_array().map(|x| x.iter().filter_map(|y| y.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        println!("{}{}", a["name"].as_str().unwrap_or_default(), if aliases.is_empty() { String::new() } else { format!(" (also: {aliases})") });
        println!("  {}{}", a["title"].as_str().unwrap_or_default(), a["by"].as_str().filter(|b| !b.is_empty()).map(|b| format!(", by {b}")).unwrap_or_default());
        println!("  model {}", a["model"].as_str().unwrap_or_default());
        if let Some(h) = a["huggingface"].as_str().filter(|h| !h.is_empty()) {
            println!("  {h}");
        }
        println!("  grounding: {}", a["grounding"].as_str().unwrap_or("none"));
    }
    let registry = d["registry"].as_str().filter(|r| !r.is_empty()).map(|r| format!("   (registry: {r})")).unwrap_or_default();
    println!("\nAsk one: w3bs ask {} \"your question\"{registry}", agents.first().and_then(|a| a["name"].as_str()).unwrap_or("business"));
}

// ------------------------------------------------------------------ chat (slash commands)

#[derive(Debug, PartialEq)]
pub enum Line {
    Ask { agent: Option<String>, question: String },
    Use(String),
    List,
    Help,
    Quit,
    Empty,
    Unknown(String),
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            return t[1..t.len() - 1].trim().to_owned();
        }
    }
    t.to_owned()
}

/// One line typed into `w3bs ask`'s chat: plain text asks the current agent; `/ask <agent>
/// <question>`, `/agent <name>`, `/list`, `/help` and `/quit` do what they say.
pub fn parse_line(line: &str) -> Line {
    let t = line.trim();
    if t.is_empty() {
        return Line::Empty;
    }
    let Some(rest) = t.strip_prefix('/') else {
        return Line::Ask { agent: None, question: unquote(t) };
    };
    let (cmd, arg) = rest.split_once(char::is_whitespace).map(|(c, a)| (c, a.trim())).unwrap_or((rest, ""));
    match cmd.to_lowercase().as_str() {
        "ask" => {
            let (agent, q) = arg.split_once(char::is_whitespace).map(|(a, q)| (a, q.trim())).unwrap_or((arg, ""));
            if agent.is_empty() { Line::Help } else { Line::Ask { agent: Some(agent.to_owned()), question: unquote(q) } }
        }
        "agent" | "use" if !arg.is_empty() => Line::Use(arg.to_owned()),
        "list" | "agents" => Line::List,
        "help" | "?" => Line::Help,
        "quit" | "exit" | "q" => Line::Quit,
        _ => Line::Unknown(cmd.to_owned()),
    }
}

pub const CHAT_HELP: &str = "\
  <question>                ask the current agent
  /ask <agent> <question>   ask another agent (and make it current)
  /agent <name>             switch agent
  /list                     the agents
  /help                     this list
  /quit                     leave (or Ctrl-D)";

/// The interactive chat on a terminal. Returns how many questions were answered.
pub async fn chat(agents: &[Agent], mut current: Agent, search: bool, max_tokens: Option<u64>, infernet: Option<&str>, timeout: Duration) -> u32 {
    use std::io::BufRead;
    eprintln!("w3bs ask: chat with W3BS agents on the c0mpute network. /help for commands, /quit to leave.");
    let stdin = std::io::stdin();
    let mut answered = 0;
    loop {
        print!("{}> ", current.name);
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            println!();
            return answered;
        }
        let (agent, question) = match parse_line(&line) {
            Line::Empty => continue,
            Line::Quit => return answered,
            Line::Help => {
                eprintln!("{CHAT_HELP}");
                continue;
            }
            Line::List => {
                print_list(&json!({ "agents": agents.iter().map(Agent::summary).collect::<Vec<_>>(), "registry": "" }));
                continue;
            }
            Line::Unknown(c) => {
                eprintln!("unknown command /{c}; /help lists them");
                continue;
            }
            Line::Use(name) => {
                match find(agents, &name) {
                    Some(a) => current = a.clone(),
                    None => eprintln!("{}", unknown(agents, &name).info().message),
                }
                continue;
            }
            Line::Ask { agent, question } => (agent, question),
        };
        if let Some(name) = agent {
            match find(agents, &name) {
                Some(a) => current = a.clone(),
                None => {
                    eprintln!("{}", unknown(agents, &name).info().message);
                    continue;
                }
            }
        }
        if question.is_empty() {
            continue;
        }
        let mut tok = |t: &str| {
            print!("{t}");
            let _ = std::io::stdout().flush();
        };
        let mut status = |m: &str| eprintln!("w3bs: {m}");
        match ask(Request { agent: &current, question: &question, search, max_tokens, infernet, timeout }, &mut tok, &mut status).await {
            Ok(d) => {
                println!();
                for w in d["warnings"].as_array().into_iter().flatten() {
                    eprintln!("warning: {}", w.as_str().unwrap_or_default());
                }
                print_footer(&d);
                answered += 1;
            }
            Err(e) => eprintln!("\nerror: {} ({})", e.info().message, e.info().code),
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agents() -> Vec<Agent> {
        parse_registry(BUILTIN).unwrap()
    }

    #[test]
    fn builtin_registry_has_the_business_agent() {
        let a = agents();
        let b = find(&a, "business").unwrap();
        assert_eq!(b.model, "hf.co/profullstack/saas-business-strategist-agent-GGUF:latest");
        assert!(b.system.starts_with("You are saas-business-strategist-agent."));
        assert!(b.search && b.results == 3);
        for alias in ["saas", "Strategist", "@business"] {
            assert_eq!(find(&a, alias).unwrap().name, "business", "{alias}");
        }
        assert!(find(&a, "nobody").is_none());
        assert!(b.disclaimer.is_empty() && b.summary().get("disclaimer").is_none());
    }

    #[test]
    fn builtin_registry_has_the_legal_and_tax_agents_with_disclaimers() {
        let a = agents();
        for (alias, name, repo, note) in [("lawyer", "legal", "not-a-lawyer-agent", "Not a lawyer."), ("@IRS", "tax", "tax-guy-agent", "Not a CPA.")] {
            let ag = find(&a, alias).unwrap();
            assert_eq!(ag.name, name);
            assert_eq!(ag.model, format!("hf.co/profullstack/{repo}-GGUF:latest"));
            assert!(ag.disclaimer.starts_with(note), "{}", ag.disclaimer);
            assert_eq!(ag.summary()["disclaimer"], ag.disclaimer.as_str());
        }
    }

    #[test]
    fn registry_skips_bad_agents_and_rejects_empty() {
        let doc = r#"{"agents":[{"name":"Bad Name","model":"m","system":"s"},{"name":"ok","model":"m","system":"s"},{"name":"nomodel","system":"s"}]}"#;
        let a = parse_registry(doc).unwrap();
        assert_eq!(a.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["ok"]);
        assert_eq!((a[0].max_tokens, a[0].search), (350, false));
        assert!(parse_registry(r#"{"agents":[]}"#).is_err());
        assert!(parse_registry("nope").is_err());
    }

    #[test]
    fn query_calms_punctuation_and_adds_the_suffix() {
        assert_eq!(search_query("What do I do, I can't make money???? HELP!", "startup revenue"), "What do I do, I can't make money? HELP! startup revenue");
        assert_eq!(search_query("  a   b ", ""), "a b");
    }

    #[test]
    fn sources_prefer_snippets_and_skip_junk() {
        let body = json!({ "results": [
            { "title": "No snippet", "url": "https://a.example/", "description": "" },
            { "title": "Good", "url": "https://b.example/", "description": "useful   text" },
            { "title": "", "url": "https://c.example/" },
            { "title": "Not web", "url": "javascript:alert(1)", "description": "x" },
            { "title": "Also good", "url": "https://d.example/", "description": "more" },
            { "title": "Extra", "url": "https://e.example/", "description": "more" },
        ]});
        let s = pick_sources(&body, 3);
        assert_eq!(s.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["Good", "Also good", "Extra"]);
        assert_eq!(s[0].text, "useful text");
        assert_eq!(pick_sources(&body, 8).len(), 4);
        assert!(pick_sources(&json!({}), 3).is_empty());
    }

    #[test]
    fn prompt_numbers_the_background() {
        let a = agents();
        let b = find(&a, "business").unwrap();
        let src = vec![Source { title: "T".into(), url: "https://t.example/".into(), text: "body".into() }];
        let m = messages(b, "Q?", &src, 0);
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[1]["content"], "Question: Q?\n\nNotes from the web:\n[1] T (https://t.example/)\nbody\n\nAnswer the question in a few practical steps.");
        assert_eq!(strip_echo(&format!("{} Do X.", b.system), &b.system), "Do X.");
        assert_eq!(strip_echo(&b.system, &b.system), "");
        assert_eq!(messages(b, "Q?", &[], 0)[1]["content"], "Q?");
        let m1 = messages(b, "Q?", &src, 1);
        assert_eq!(m1[1]["content"], "Q?");
        assert!(m1[0]["content"].as_str().unwrap().ends_with("Notes from the web you may use:\n[1] T (https://t.example/)\nbody"));
    }

    #[test]
    fn echo_guard_hides_a_repeated_system_prompt() {
        let sys = "You are x. Be brief.";
        let mut g = EchoGuard::default();
        assert_eq!(g.push("You are", sys), None);
        assert_eq!(g.push(" x. Be brief.", sys), None);
        assert_eq!(g.push(" Do A.", sys).as_deref(), Some("Do A."));
        assert_eq!(g.push(" Then B.", sys).as_deref(), Some(" Then B."));
        let mut g = EchoGuard::default();
        assert_eq!(g.push("You ", sys), None);
        assert_eq!(g.push("can sell.", sys).as_deref(), Some("You can sell."));
    }

    #[test]
    fn sse_frames_split_across_chunks() {
        let mut p = Sse::default();
        assert!(p.push("event: meta\ndata: {\"provider_name\":\"n1\"}\n").is_empty());
        let f = p.push("\nid: 3\nevent: token\r\ndata: {\"text\":\"Hi\"}\r\n\r\nevent: done\ndata: {\"text\":\"Hi there\"}\n\n: keepalive\n\n");
        assert_eq!(f.len(), 3);
        assert_eq!((f[0].0.as_str(), f[0].1["provider_name"].as_str()), ("meta", Some("n1")));
        assert_eq!((f[1].0.as_str(), f[1].1["text"].as_str()), ("token", Some("Hi")));
        assert_eq!(f[2].0, "done");
    }

    #[test]
    fn chat_lines() {
        assert_eq!(parse_line("/ask business \"What do I do?\""), Line::Ask { agent: Some("business".into()), question: "What do I do?".into() });
        assert_eq!(parse_line("how do I price?"), Line::Ask { agent: None, question: "how do I price?".into() });
        assert_eq!(parse_line("/agent saas"), Line::Use("saas".into()));
        assert_eq!(parse_line("/LIST"), Line::List);
        assert_eq!(parse_line("/ask"), Line::Help);
        assert_eq!(parse_line("/quit"), Line::Quit);
        assert_eq!(parse_line("   "), Line::Empty);
        assert_eq!(parse_line("/nope x"), Line::Unknown("nope".into()));
    }

    #[test]
    fn only_https_or_loopback() {
        assert!(checked_url("https://infernetprotocol.com", "x").is_ok());
        assert!(checked_url("http://127.0.0.1:9", "x").is_ok());
        assert!(checked_url("http://infernetprotocol.com", "x").is_err());
    }
}
