//! `w3bs ab`: your A/B tests on w3bs.org/ab-testing (OpenABTest 0.2 draft).
//!
//! Every command uses the account from `w3bs login`: list, create, start, pause, resume,
//! close and delete experiments, read their results, export the OpenABTest manifest and
//! events, and ask for a participant's variant the way a server would (`w3bs ab assign`).
//! Servers in production use an `ab_` key from the page instead; see
//! w3bs.org/docs/ab-testing.md.

use crate::auth;
use serde_json::{Value, json};
use std::time::Duration;
use w3bs_core::{Error, Result};

fn http() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build().expect("http client")
}

async fn call(origin: &str, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
    let token = auth::access_token(origin).await?;
    let url = format!("{origin}{path}");
    let mut req = http().request(method, &url).bearer_auth(token);
    if let Some(body) = body {
        req = req.json(body);
    }
    let resp = req.send().await.map_err(|e| Error::package("AB_UNAVAILABLE", format!("{url}: {e}")).retryable())?;
    let status = resp.status().as_u16();
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default()).unwrap_or(Value::Null);
    match status {
        200..=299 => Ok(v),
        401 => Err(Error::package("NOT_SIGNED_IN", format!("Your session has ended. Run: w3bs login{}", if origin == "https://w3bs.org" { String::new() } else { format!(" --api {origin}") }))),
        _ => {
            let code = v["error"]["code"].as_str().unwrap_or("AB_REQUEST_FAILED");
            let msg = v["error"]["message"].as_str().map(str::to_owned).unwrap_or_else(|| format!("the server answered {status}"));
            Err(Error::package(if code == "NO_SUCH_EXPERIMENT" { "NO_SUCH_EXPERIMENT" } else { "AB_REQUEST_FAILED" }, msg))
        }
    }
}

fn path_id(slug: &str) -> Result<&str> {
    if !slug.is_empty() && slug.len() <= 64 && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        Ok(slug)
    } else {
        Err(Error::usage("BAD_EXPERIMENT_ID", format!("{slug:?} is not an experiment id (lowercase letters, digits and dashes); see `w3bs ab ls`")))
    }
}

/// `--variant` values: `id`, `id:weight`, `id={"json":"parameters"}` or `id:weight={...}`.
pub fn parse_variant(spec: &str) -> Result<Value> {
    let (head, params) = match spec.split_once('=') {
        Some((h, p)) => (h, Some(p)),
        None => (spec, None),
    };
    let (id, weight) = match head.split_once(':') {
        Some((id, w)) => (id, w.parse::<u32>().map_err(|_| Error::usage("BAD_VARIANT", format!("{spec:?}: the weight after ':' must be a whole number")))?),
        None => (head, 1),
    };
    if id.is_empty() {
        return Err(Error::usage("BAD_VARIANT", format!("{spec:?}: give the variant an id")));
    }
    let mut v = json!({ "id": id, "weight": weight });
    if let Some(p) = params {
        let parsed: Value = serde_json::from_str(p).map_err(|e| Error::usage("BAD_VARIANT", format!("{spec:?}: the parameters are not JSON: {e}")))?;
        if !parsed.is_object() {
            return Err(Error::usage("BAD_VARIANT", format!("{spec:?}: the parameters must be a JSON object")));
        }
        v["parameters"] = parsed;
    }
    Ok(v)
}

pub async fn ls(origin: &str) -> Result<Value> {
    let list = call(origin, reqwest::Method::GET, "/api/ab/experiments", None).await?;
    Ok(json!({ "origin": origin, "experiments": list }))
}

pub async fn show(origin: &str, slug: &str) -> Result<Value> {
    let r = call(origin, reqwest::Method::GET, &format!("/api/ab/experiments/{}", path_id(slug)?), None).await?;
    Ok(json!({ "origin": origin, "results": r }))
}

pub struct NewExperiment {
    pub slug: String,
    pub name: Option<String>,
    pub variants: Vec<String>,
    pub unit: Option<String>,
    pub days: Option<u32>,
    pub min: Option<u32>,
    pub eligible: Option<String>,
}

pub async fn new(origin: &str, x: NewExperiment) -> Result<Value> {
    if x.variants.len() < 2 {
        return Err(Error::usage("BAD_VARIANT", "give at least two --variant, the first is the control"));
    }
    let variants = x.variants.iter().map(|v| parse_variant(v)).collect::<Result<Vec<_>>>()?;
    let mut body = json!({ "slug": path_id(&x.slug)?, "name": x.name.clone().unwrap_or_else(|| x.slug.clone()), "variants": variants });
    if let Some(u) = x.unit {
        body["unit"] = json!(u);
    }
    if let Some(d) = x.days {
        body["days"] = json!(d);
    }
    if let Some(m) = x.min {
        body["minimumEligiblePerVariant"] = json!(m);
    }
    if let Some(e) = x.eligible {
        body["eligibilityRule"] = json!(e);
    }
    let made = call(origin, reqwest::Method::POST, "/api/ab/experiments", Some(&body)).await?;
    Ok(json!({ "origin": origin, "experiment": made }))
}

pub async fn set_state(origin: &str, slug: &str, state: &str) -> Result<Value> {
    let x = call(origin, reqwest::Method::POST, &format!("/api/ab/experiments/{}/state", path_id(slug)?), Some(&json!({ "state": state }))).await?;
    Ok(json!({ "origin": origin, "experiment": x }))
}

pub async fn rm(origin: &str, slug: &str) -> Result<Value> {
    let r = call(origin, reqwest::Method::DELETE, &format!("/api/ab/experiments/{}", path_id(slug)?), None).await?;
    Ok(json!({ "origin": origin, "removed": r["removed"], "slug": slug }))
}

pub async fn assign(origin: &str, slug: &str, participant: &str) -> Result<Value> {
    let r = call(origin, reqwest::Method::POST, "/api/ab/assign", Some(&json!({ "experiment": path_id(slug)?, "participant": participant }))).await?;
    Ok(json!({ "origin": origin, "assignment": r }))
}

pub async fn export(origin: &str, slug: &str, events: bool) -> Result<Value> {
    let file = if events { "events.json" } else { "manifest.json" };
    let doc = call(origin, reqwest::Method::GET, &format!("/api/ab/experiments/{}/{file}", path_id(slug)?), None).await?;
    Ok(json!({ "origin": origin, "document": doc }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_flags() {
        assert_eq!(parse_variant("control").unwrap(), json!({ "id": "control", "weight": 1 }));
        assert_eq!(parse_variant("green:3").unwrap(), json!({ "id": "green", "weight": 3 }));
        assert_eq!(parse_variant(r#"green={"color":"green"}"#).unwrap(), json!({ "id": "green", "weight": 1, "parameters": { "color": "green" } }));
        assert_eq!(parse_variant(r#"b:2={"x":1}"#).unwrap(), json!({ "id": "b", "weight": 2, "parameters": { "x": 1 } }));
        assert!(parse_variant("a:x").is_err());
        assert!(parse_variant("a=[1]").is_err());
        assert!(parse_variant("a={nope").is_err());
        assert!(parse_variant(":2").is_err());
    }

    #[test]
    fn experiment_ids() {
        assert!(path_id("checkout-button").is_ok());
        assert!(path_id("../keys").is_err());
        assert!(path_id("Upper").is_err());
        assert!(path_id("").is_err());
    }
}
