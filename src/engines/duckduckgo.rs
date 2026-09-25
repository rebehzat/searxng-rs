//! DuckDuckGo HTML adapter (`https://html.duckduckgo.com/html/`).
//!
//! This adapter performs a plain HTTP GET of the server-rendered results
//! page and parses it with CSS selectors. It relies on the default,
//! unauthenticated HTML endpoint and deliberately implements **no**
//! CAPTCHA/anti-bot bypasses: if DDG starts challenging the client the
//! engine reports zero results with a soft error instead.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;
use scraper::{Html, Selector};
use tracing::debug;

use crate::engines::Engine;
use crate::error::{EngineError, EngineResult};
use crate::models::SearchResult;

pub const ENDPOINT: &str = "https://html.duckduckgo.com/html/";

/// DuckDuckGo "HTML" endpoint adapter.
pub struct DuckDuckGoHtml {
    name: String,
    client: reqwest::Client,
    region: String,
}

impl DuckDuckGoHtml {
    pub fn new(name: &str, user_agent: &str, region: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(user_agent.to_string())
            .build()
            .expect("static client configuration");
        Self {
            name: name.to_string(),
            client,
            region: region.to_string(),
        }
    }

    /// Build the request URL for a query.
    pub fn build_url(&self, query: &str) -> Url {
        let mut url = Url::parse(ENDPOINT).expect("static endpoint is a valid URL");
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("kl", &self.region);
        url
    }

    /// Extract `(title, url, snippet)` triples from the results HTML.
    ///
    /// Public for unit testing with saved fixtures.
    pub fn parse_html(&self, html: &str) -> Vec<(String, String, Option<String>)> {
        let doc = Html::parse_document(html);
        let result_sel = Selector::parse(".result").expect("valid selector");
        let link_sel = Selector::parse("a.result__a").expect("valid selector");
        let snippet_sel = Selector::parse(".result__snippet").expect("valid selector");

        let mut out = Vec::new();
        for element in doc.select(&result_sel) {
            let Some(link) = element.select(&link_sel).next() else {
                continue;
            };
            let title = normalize_ws(&link.text().collect::<String>());
            if title.is_empty() {
                continue;
            }
            let raw_href = link.value().attr("href").unwrap_or_default();
            let url = match absolutize_ddg_href(raw_href) {
                Some(u) => u,
                None => continue,
            };
            let snippet = element
                .select(&snippet_sel)
                .next()
                .map(|s| normalize_ws(&s.text().collect::<String>()))
                .filter(|s| !s.is_empty());
            out.push((title, url, snippet));
        }
        out
    }
}

#[async_trait]
impl Engine for DuckDuckGoHtml {
    fn name(&self) -> &str {
        &self.name
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
        timeout: Duration,
    ) -> EngineResult<Vec<SearchResult>> {
        let url = self.build_url(query);
        debug!(engine = self.name(), %url, "requesting");

        let resp = self
            .client
            .get(url)
            .timeout(timeout)
            .send()
            .await?
            .error_for_status()?;

        let body = resp.text().await?;
        let parsed = self.parse_html(&body);
        if parsed.is_empty() {
            // Either the query truly has no hits, or DDG served a challenge
            // page. We cannot (and will not) tell them apart definitively;
            // report a soft parse failure.
            return Err(EngineError::Parse);
        }

        Ok(parsed
            .into_iter()
            .take(limit)
            .map(|(title, url, snippet)| SearchResult::new(self.name(), title, url, snippet))
            .collect())
    }
}

/// Normalize whitespace collapsed by HTML formatting.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Resolve DuckDuckGo redirect links (`//duckduckgo.com/l/?uddg=...`) to the
/// real target, and make protocol-relative URLs absolute.
fn absolutize_ddg_href(href: &str) -> Option<String> {
    if href.is_empty() {
        return None;
    }

    if let Some(decoded) = decode_uddg(href) {
        return web_url(decoded);
    }

    if href.starts_with("//") {
        return web_url(format!("https:{href}"));
    }
    web_url(href)
}

/// Accept only absolute HTTP(S) URLs suitable for normalized search results.
fn web_url(candidate: impl AsRef<str>) -> Option<String> {
    let url = Url::parse(candidate.as_ref()).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

/// Extract and percent-decode the `uddg` query parameter of a DDG redirect.
fn decode_uddg(href: &str) -> Option<String> {
    let path = href.strip_prefix("//").unwrap_or(href);
    let idx = path.find("/l/?")?;
    let query = &path[idx + "/l/?".len()..];
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == "uddg" {
            let decoded = urlencoding::decode(v).ok()?;
            return Some(decoded.into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
<html><body>
<div class="results">
  <div class="result results_links results_links_deep web-result">
    <h2 class="result__title">
      <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">Rust Programming Language</a>
    </h2>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">
      A language empowering everyone
      to build reliable and efficient software.
    </a>
  </div>
  <div class="result results_links results_links_deep web-result">
    <h2 class="result__title">
      <a rel="nofollow" class="result__a" href="https://doc.rust-lang.org/book/">The Rust Book</a>
    </h2>
    <div class="result__snippet">Learn Rust with a hands-on guide.</div>
  </div>
  <div class="result results_links results_links_deep web-result">
    <h2 class="result__title"><a class="result__a" href=""></a></h2>
  </div>
</div>
</body></html>
"#;

    #[test]
    fn build_url_encodes_query_and_region() {
        let e = DuckDuckGoHtml::new("ddg_html", "ua/test", "wt-wt");
        let url = e.build_url("rust & \"lang\"");
        let s = url.as_str();
        assert!(s.starts_with(super::ENDPOINT));
        assert!(s.contains("q=rust+%26+%22lang%22"), "got: {s}");
        assert!(s.contains("kl=wt-wt"));
    }

    #[test]
    fn parses_fixture_results() {
        let e = DuckDuckGoHtml::new("ddg_html", "ua/test", "wt-wt");
        let results = e.parse_html(FIXTURE);
        assert_eq!(results.len(), 2, "empty-title block must be skipped");

        let (title, url, snippet) = &results[0];
        assert_eq!(title, "Rust Programming Language");
        assert_eq!(url, "https://www.rust-lang.org/");
        assert!(
            snippet
                .as_deref()
                .unwrap()
                .starts_with("A language empowering")
        );

        let (title, url, snippet) = &results[1];
        assert_eq!(title, "The Rust Book");
        assert_eq!(url, "https://doc.rust-lang.org/book/");
        assert_eq!(
            snippet.as_deref(),
            Some("Learn Rust with a hands-on guide.")
        );
    }

    #[test]
    fn parse_of_junk_is_empty() {
        let e = DuckDuckGoHtml::new("ddg_html", "ua/test", "wt-wt");
        assert!(e.parse_html("<html><body>blocked</body></html>").is_empty());
    }

    #[test]
    fn decodes_uddg_redirects_and_absolute_urls() {
        assert_eq!(
            decode_uddg("//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.example%2Fb&rut=x").as_deref(),
            Some("https://a.example/b")
        );
        assert_eq!(decode_uddg("https://example.com/"), None);
        assert_eq!(
            absolutize_ddg_href("//example.com/x").as_deref(),
            Some("https://example.com/x")
        );
        assert_eq!(
            absolutize_ddg_href("https://example.com/y").as_deref(),
            Some("https://example.com/y")
        );
        assert_eq!(absolutize_ddg_href(""), None);
    }

    #[test]
    fn rejects_non_web_redirect_targets() {
        assert_eq!(
            absolutize_ddg_href("//duckduckgo.com/l/?uddg=javascript%3Aalert%281%29"),
            None
        );
        assert_eq!(
            absolutize_ddg_href("//duckduckgo.com/l/?uddg=file%3A%2F%2F%2Fetc%2Fpasswd"),
            None
        );
        assert_eq!(absolutize_ddg_href("data:text/html,not-a-result"), None);
    }
}
