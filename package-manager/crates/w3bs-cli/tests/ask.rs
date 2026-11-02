//! `w3bs ask` against a fake agent registry, pit.world search and Infernet chat API.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// (method, path, JSON body, authorization header) of each request.
type Seen = Arc<Mutex<Vec<(String, String, Value, String)>>>;

struct Fake {
    origin: String,
    seen: Seen,
}

/// `chat_status` is what POST /api/chat answers (200 routes to a node, 429 is the rate limit).
fn fake(chat_status: u16) -> Fake {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    let posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if r.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (method, path) = (parts.next().unwrap_or("").to_owned(), parts.next().unwrap_or("").to_owned());
            let (mut len, mut auth) = (0usize, String::new());
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
                let lower = h.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                if let Some(v) = lower.strip_prefix("authorization:") {
                    auth = v.trim().to_owned();
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            log.lock().unwrap().push((method.clone(), path.clone(), body, auth));
            let route = path.split('?').next().unwrap_or("").to_owned();
            let (status, ctype, out): (u16, &str, String) = match (method.as_str(), route.as_str()) {
                ("GET", "/agents.json") => (200, "application/json", json!({ "schema": 1, "agents": [
                    { "name": "business", "aliases": ["saas"], "title": "Strategist", "model": "hf.co/x/y-GGUF:latest", "system": "You are y.",
                      "grounding": { "search": "pit.world", "query_suffix": "startup", "results": 2 }, "max_tokens": 99, "temperature": 0.1 },
                    { "name": "plain", "model": "m:1", "system": "You are plain." }
                ]}).to_string()),
                ("GET", "/search") => (200, "application/json", json!({ "results": [
                    { "title": "One", "url": "https://one.example/", "description": "first" },
                    { "title": "Two", "url": "https://two.example/", "description": "second" },
                    { "title": "Three", "url": "https://three.example/", "description": "third" }
                ]}).to_string()),
                ("POST", "/api/chat") if chat_status == 429 => (429, "application/json", json!({ "error": "Rate limit exceeded", "detail": { "resetAt": "2026-11-02T13:00:00.000Z" } }).to_string()),
                ("POST", "/api/chat") => {
                    let n = posts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    (200, "application/json", json!({ "jobId": format!("j{n}"), "status": "assigned", "source": "p2p", "model": "hf.co/x/y-GGUF:latest",
                        "provider": { "id": "p1", "name": "node-one", "nodeId": "provider-1" }, "streamUrl": format!("/api/chat/stream/j{n}") }).to_string())
                }
                // Every odd job only repeats the business agent's system prompt; the CLI must
                // hide it and ask again.
                ("GET", r) if r.starts_with("/api/chat/stream/j") && r[18..].parse::<usize>().unwrap_or(0) % 2 == 1 => (200, "text/event-stream", [
                    "event: token\ndata: {\"text\":\"You are \"}\n\n",
                    "event: token\ndata: {\"text\":\"y.\"}\n\n",
                    "event: done\ndata: {\"text\":\"You are y.\",\"reason\":\"stop\"}\n\n",
                ].concat()),
                ("GET", r) if r.starts_with("/api/chat/stream/j") => (200, "text/event-stream", [
                    "event: job\ndata: {\"id\":\"j1\"}\n\n",
                    "event: meta\ndata: {\"provider_node_id\":\"provider-1\",\"provider_name\":\"node-one\",\"engine\":\"ollama\"}\n\n",
                    "event: token\ndata: {\"text\":\"Find \"}\n\n",
                    "event: token\ndata: {\"text\":\"customers.\"}\n\n",
                    "event: done\ndata: {\"text\":\"Find customers.\",\"reason\":\"stop\"}\n\n",
                ].concat()),
                _ => (404, "application/json", "{}".into()),
            };
            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
        }
    });
    Fake { origin, seen }
}

fn w3bs(f: &Fake, args: &[&str], extra: &[(&str, &str)]) -> (i32, String, String) {
    let home = tempfile::tempdir().unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_w3bs"));
    c.args(args)
        .env("W3BS_HOME", home.path())
        .env("W3BS_AGENTS", format!("{}/agents.json", f.origin))
        .env("W3BS_SEARCH_URL", format!("{}/search", f.origin))
        .env("W3BS_INFERNET_API", &f.origin)
        .env_remove("INFERNET_API_KEY");
    for (k, v) in extra {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

#[test]
fn ask_grounds_routes_p2p_and_streams() {
    let f = fake(200);
    let (code, out, err) = w3bs(&f, &["ask", "saas", "How", "do", "I", "make", "money??", "--json"], &[]);
    assert_eq!(code, 0, "{out} {err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let d = &v["data"];
    assert_eq!(d["agent"], "business");
    assert_eq!(d["answer"], "Find customers.");
    assert_eq!(d["node"]["name"], "node-one");
    assert_eq!(d["node"]["engine"], "ollama");
    assert_eq!(d["sources"].as_array().unwrap().len(), 2);
    assert_eq!(d["sources"][1]["url"], "https://two.example/");
    assert_eq!(d["registry"], format!("{}/agents.json", f.origin));

    let seen = f.seen.lock().unwrap().clone();
    let search = seen.iter().find(|(_, p, _, _)| p.starts_with("/search")).unwrap();
    assert_eq!(search.1, "/search?q=How+do+I+make+money%3F+startup");
    let posts: Vec<_> = seen.iter().filter(|(m, p, _, _)| m == "POST" && p == "/api/chat").collect();
    assert_eq!(posts.len(), 2, "the echo was retried once");
    assert_eq!(posts[1].2["messages"][1]["content"], "How do I make money??", "the retry moves the background into the system message");
    assert!(posts[1].2["messages"][0]["content"].as_str().unwrap().contains("Notes from the web you may use:"));
    assert_eq!(d["job_id"], "j2");
    let (_, _, chat, auth) = posts[0];
    assert_eq!(chat["p2p_only"], true);
    assert_eq!(chat["modelName"], "hf.co/x/y-GGUF:latest");
    assert_eq!((chat["maxTokens"].as_u64(), chat["temperature"].as_f64()), (Some(99), Some(0.1)));
    assert_eq!(chat["messages"][0]["content"], "You are y.");
    let user = chat["messages"][1]["content"].as_str().unwrap();
    assert!(user.starts_with("Question: How do I make money??") && user.contains("[2] Two (https://two.example/)\nsecond"), "{user}");
    assert!(auth.is_empty(), "anonymous by default");

    // Human output: the streamed answer, then the sources; who served it goes to stderr.
    let (code, out, err) = w3bs(&f, &["ask", "business", "Help?"], &[("INFERNET_API_KEY", "ifc_test")]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.starts_with("Find customers.\n"), "the echo never reaches the screen: {out}");
    assert!(out.contains("Sources:\n  [1] One\n      https://one.example/"), "{out}");
    assert!(err.contains("on c0mpute node node-one (provider-1)"), "{err}");
    let last = f.seen.lock().unwrap().iter().rev().find(|(m, p, _, _)| m == "POST" && p == "/api/chat").unwrap().3.clone();
    assert_eq!(last, "bearer ifc_test");
}

#[test]
fn ask_no_search_list_and_errors() {
    let f = fake(200);
    let (code, out, _) = w3bs(&f, &["ask", "plain", "hi", "--no-search", "--json"], &[]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["data"]["sources"].as_array().unwrap().is_empty());
    assert!(!f.seen.lock().unwrap().iter().any(|(_, p, _, _)| p.starts_with("/search")));

    let (code, out, _) = w3bs(&f, &["ask", "--list", "--json"], &[]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["agents"][0]["name"], "business");
    assert_eq!(v["data"]["agents"][0]["aliases"], json!(["saas"]));

    let (code, out, _) = w3bs(&f, &["ask", "nobody", "hi", "--json"], &[]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("ASK_UNKNOWN_AGENT"));

    let (code, out, _) = w3bs(&f, &["ask", "business", "--json"], &[]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("ASK_NO_QUESTION"));

    let limited = fake(429);
    let (code, out, _) = w3bs(&limited, &["ask", "business", "hi", "--json"], &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("ASK_RATE_LIMITED") && out.contains("INFERNET_API_KEY"), "{out}");
}

#[test]
fn ask_falls_back_to_the_built_in_registry() {
    let f = fake(200);
    let (code, out, _) = w3bs(&f, &["ask", "--list", "--json"], &[("W3BS_AGENTS", "http://127.0.0.1:9/agents.json")]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["registry"], "built-in");
    assert_eq!(v["data"]["agents"][0]["model"], "hf.co/profullstack/saas-business-strategist-agent-GGUF:latest");
    assert!(v["warnings"][0].as_str().unwrap().contains("built into this w3bs"));
}
