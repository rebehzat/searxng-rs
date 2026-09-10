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
                Ok(u) if u.as_str().starts_with("http") => u.to_string(),
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
}
