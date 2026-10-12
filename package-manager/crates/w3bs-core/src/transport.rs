//! Network transport for TUF fetches.
//!
//! Production: HTTPS only, certificates verified, at most five redirects, and a redirect
//! to anything that is not HTTPS is refused. No credentials are ever attached, so none
//! can leak across an origin change.
//!
//! Development (`W3BS_DEV_INSECURE_TRANSPORT=1` with an explicit `W3BS_HOME`): additionally
//! `file://` and `http://` to loopback hosts, for tests and local repositories.

use async_trait::async_trait;
use futures::TryStreamExt;
use reqwest::redirect::Policy;
use std::time::Duration;
use tough::{FilesystemTransport, Transport, TransportError, TransportErrorKind, TransportStream};
use url::Url;

pub const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone)]
pub struct W3bsTransport {
    client: reqwest::Client,
    dev: bool,
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Whether a repository URL is acceptable for this mode.
pub fn url_allowed(url: &Url, dev: bool) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => dev && is_loopback(url),
        "file" => dev,
        _ => false,
    }
}

impl W3bsTransport {
    pub fn new(dev: bool) -> Self {
        let policy = Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if !url_allowed(attempt.url(), dev) || attempt.url().scheme() == "file" {
                attempt.error(format!("refusing redirect to {}", attempt.url()))
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .redirect(policy)
            .https_only(!dev)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .user_agent(concat!("w3bs/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Self { client, dev }
    }
}

#[async_trait]
impl Transport for W3bsTransport {
    async fn fetch(&self, url: Url) -> Result<TransportStream, TransportError> {
        if !url_allowed(&url, self.dev) {
            return Err(TransportError::new(TransportErrorKind::UnsupportedUrlScheme, url));
        }
        if url.scheme() == "file" {
            return FilesystemTransport.fetch(url).await;
        }
        let resp = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|e| TransportError::new_with_cause(TransportErrorKind::Other, url.clone(), e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::FORBIDDEN {
            return Err(TransportError::new(TransportErrorKind::FileNotFound, url));
        }
        if !status.is_success() {
            return Err(TransportError::new_with_cause(
                TransportErrorKind::Other,
                url.clone(),
                std::io::Error::other(format!("HTTP {status}")),
            ));
        }
        let u = url.clone();
        let stream = resp.bytes_stream().map_err(move |e| TransportError::new_with_cause(TransportErrorKind::Other, u.clone(), e));
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_rules() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(url_allowed(&u("https://tools.example/repo/"), false));
        assert!(!url_allowed(&u("http://tools.example/repo/"), false));
        assert!(!url_allowed(&u("http://127.0.0.1:8080/"), false));
        assert!(url_allowed(&u("http://127.0.0.1:8080/"), true));
        assert!(!url_allowed(&u("http://tools.example/"), true));
        assert!(!url_allowed(&u("file:///tmp/repo/"), false));
        assert!(url_allowed(&u("file:///tmp/repo/"), true));
        assert!(!url_allowed(&u("ftp://tools.example/"), true));
    }
}
