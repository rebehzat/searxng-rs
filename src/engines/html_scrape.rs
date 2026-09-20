//! Generic server-rendered HTML results adapter.
//!
//! One configurable backend for search engines that expose plain HTML
//! result lists (Bing, Brave, Startpage, Mojeek, Google's HTML endpoint,
//! ...). Selectors are provided per engine via configuration, so a site
//! redesign is usually a config change rather than a code change.
//!
//! Like the DuckDuckGo adapter, this backend performs plain HTTP GETs and
//! implements **no** CAPTCHA/anti-bot bypasses: engines that challenge the
//! client simply report a soft parse error.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;
use scraper::{Html, Selector};
use tracing::debug;

use crate::config::EngineConfig;
use crate::engines::Engine;
use crate::error::{EngineError, EngineResult};
use crate::models::SearchResult;

/// Selector-driven adapter over a server-rendered results page.
pub struct HtmlScrape {
    name: String,
    client: reqwest::Client,
    endpoint: String,
    query_param: String,
    extra_params: Vec<(String, String)>,
    result_selector: Selector,
    link_selector: Selector,
    snippet_selector: Option<Selector>,
    link_url_attr: String,
}

/// Collapse runs of whitespace into single spaces and trim.
fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;

    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Some(out)
}

fn decode_bing_redirect(url: &Url) -> Option<Url> {
    if url.host_str()? != "www.bing.com" || url.path() != "/ck/a" {
        return None;
    }

    let encoded = url.query_pairs().find(|(key, _)| key == "u")?.1;
    let encoded = encoded.strip_prefix("a1")?;
    let decoded = decode_base64url(encoded)?;
    let target = Url::parse(std::str::from_utf8(&decoded).ok()?).ok()?;

    matches!(target.scheme(), "http" | "https").then_some(target)
}

impl HtmlScrape {
    pub fn from_config(name: &str, cfg: &EngineConfig, user_agent: &str) -> anyhow::Result<Self> {
        let endpoint = cfg.string_param("endpoint", "");
        if endpoint.is_empty() {
            anyhow::bail!("engine '{name}' requires an endpoint");
        }
        let result_selector = cfg.string_param("result_selector", "");
        if result_selector.is_empty() {
            anyhow::bail!("engine '{name}' requires a result_selector");
        }
        let link_selector = cfg.string_param("link_selector", "");
        if link_selector.is_empty() {
            anyhow::bail!("engine '{name}' requires a link_selector");
        }
        let snippet_selector = cfg.string_param("snippet_selector", "");
        let extra_params = {
            let mut params: Vec<(String, String)> = cfg
                .params
                .iter()
                .filter_map(|(key, value)| {
                    let raw = key.strip_prefix("param_")?;
                    let value = value.as_str()?;
                    (!raw.is_empty() || !value.is_empty())
                        .then(|| (raw.to_string(), value.to_string()))
                })
                .collect();
            params.sort();
            params
        };
        Ok(Self {
            name: name.into(),
            client: reqwest::Client::builder().user_agent(user_agent).build()?,
            endpoint,
            query_param: cfg.string_param("query_param", "q"),
            result_selector: Selector::parse(&result_selector)
                .map_err(|e| anyhow::anyhow!("engine '{name}': invalid result_selector: {e:?}"))?,
            link_selector: Selector::parse(&link_selector)
                .map_err(|e| anyhow::anyhow!("engine '{name}': invalid link_selector: {e:?}"))?,
            snippet_selector: (!snippet_selector.is_empty())
                .then(|| {
                    Selector::parse(&snippet_selector).map_err(|e| {
                        anyhow::anyhow!("engine '{name}': invalid snippet_selector: {e:?}")
                    })
                })
                .transpose()?,
            link_url_attr: cfg.string_param("link_url_attr", "href"),
            extra_params,
        })
    }

    /// Build the request URL for a query.
    pub fn build_url(&self, query: &str) -> Url {
        let mut url = Url::parse(&self.endpoint).expect("configured endpoint is a valid URL");
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair(&self.query_param, query);
            for (key, value) in &self.extra_params {
                pairs.append_pair(key, value);
            }
        }
        url
    }

    /// Extract `(title, url, snippet)` triples from the results HTML.
    ///
    /// Public for unit testing with saved fixtures.
    pub fn parse_html(&self, base: &Url, html: &str) -> Vec<(String, String, Option<String>)> {
        let doc = Html::parse_document(html);
        let mut out = Vec::new();
        for element in doc.select(&self.result_selector) {
            let Some(link) = element.select(&self.link_selector).next() else {
                continue;
            };
            let title = normalize_ws(&link.text().collect::<String>());
            if title.is_empty() {
                continue;
            }
            let raw_href = link.value().attr(&self.link_url_attr).unwrap_or_default();
            if raw_href.is_empty() {
                continue;
            }
            let url = match base.join(raw_href) {
                Ok(u) if matches!(u.scheme(), "http" | "https") => {
                    if self.name == "bing" {
                        decode_bing_redirect(&u).unwrap_or(u).to_string()
                    } else {
                        u.to_string()
                    }
                }
                _ => continue,
            };
            let snippet = self
                .snippet_selector
                .as_ref()
                .and_then(|sel| element.select(sel).next())
                .map(|s| normalize_ws(&s.text().collect::<String>()))
                .filter(|s| !s.is_empty());
            out.push((title, url, snippet));
        }
        out
    }
}

#[async_trait]
impl Engine for HtmlScrape {
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
            .get(url.clone())
            .timeout(timeout)
            .send()
            .await?
            .error_for_status()?;

        let body = resp.text().await?;
        let parsed = self.parse_html(&url, &body);
        if parsed.is_empty() {
            // Either no hits, or the engine served a challenge/consent page.
            // We cannot tell them apart definitively; report a soft failure.
            return Err(EngineError::Parse);
        }

        Ok(parsed
            .into_iter()
            .take(limit)
            .map(|(title, url, snippet)| SearchResult::new(self.name(), title, url, snippet))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> HtmlScrape {
        let mut params = std::collections::BTreeMap::new();
        let mut insert = |k: &str, v: &str| params.insert(k.to_string(), toml::Value::from(v));
        insert("endpoint", "https://www.bing.com/search");
        insert("query_param", "q");
        insert("param_cc", "tr");
        insert("result_selector", "li.b_algo");
        insert("link_selector", "h2 a");
        insert("snippet_selector", ".b_caption p");
        let cfg = EngineConfig {
            engine_type: "html_scrape".into(),
            enabled: true,
            params,
        };
        HtmlScrape::from_config("bing", &cfg, "searxng-rs/test").expect("valid config")
    }

    const FIXTURE: &str = r#"
    <html><body>
      <li class="b_algo">
        <h2><a href="https://rust-lang.org/learn">Learn Rust</a></h2>
        <div class="b_caption"><p>The Rust   programming language book.</p></div>
      </li>
      <li class="b_algo">
        <h2><a href="/relative/path">Relative link</a></h2>
      </li>
      <li class="b_algo"><h2><a>No href here</a></h2></li>
    </body></html>
    "#;

    #[test]
    fn builds_url_with_extra_params() {
        let engine = engine();
        let url = engine.build_url("rust async");
        assert_eq!(
            url.as_str(),
            "https://www.bing.com/search?q=rust+async&cc=tr"
        );
    }

    #[test]
    fn parses_fixture_results() {
        let engine = engine();
        let base = engine.build_url("rust");
        let results = engine.parse_html(&base, FIXTURE);
        assert_eq!(results.len(), 2, "skips entries without usable hrefs");
        assert_eq!(results[0].0, "Learn Rust");
        assert_eq!(results[0].1, "https://rust-lang.org/learn");
        assert_eq!(
            results[0].2.as_deref(),
            Some("The Rust programming language book.")
        );
        // Relative hrefs resolve against the search endpoint.
        assert_eq!(results[1].1, "https://www.bing.com/relative/path");
        assert_eq!(results[1].2, None);
    }

    #[test]
    fn decodes_bing_tracking_url() {
        let engine = engine();
        let base = engine.build_url("rust");
        let html = r#"
        <li class="b_algo">
          <h2><a href="https://www.bing.com/ck/a?u=a1aHR0cHM6Ly93d3cucnVzdC1sYW5nLm9yZy8">Rust</a></h2>
        </li>
        "#;

        let results = engine.parse_html(&base, html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "https://www.rust-lang.org/");
    }

    #[test]
    fn leaves_malformed_bing_tracking_url_unchanged() {
        let engine = engine();
        let base = engine.build_url("rust");
        let tracking = "https://www.bing.com/ck/a?u=a1%%%not-base64%%%";
        let html = format!(r#"<li class="b_algo"><h2><a href="{tracking}">Rust</a></h2></li>"#);

        let results = engine.parse_html(&base, &html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, tracking);
    }

    #[test]
    fn rejects_non_http_bing_redirect_target() {
        let url = Url::parse("https://www.bing.com/ck/a?u=a1amF2YXNjcmlwdDphbGVydCgxKQ").unwrap();
        assert!(decode_bing_redirect(&url).is_none());
    }

    #[test]
    fn ignores_non_bing_redirect_urls() {
        let url =
            Url::parse("https://example.com/ck/a?u=a1aHR0cHM6Ly93d3cucnVzdC1sYW5nLm9yZy8").unwrap();
        assert!(decode_bing_redirect(&url).is_none());
    }
}
