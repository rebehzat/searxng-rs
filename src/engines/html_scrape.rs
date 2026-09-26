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

use std::collections::HashSet;
use std::time::{Duration, Instant};

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
    path_query: bool,
    query_param: String,
    extra_params: Vec<(String, String)>,
    result_selector: Selector,
    link_selector: Selector,
    title_selector: Option<Selector>,
    title_attribute: Option<String>,
    snippet_selector: Option<Selector>,
    link_url_attr: String,
    /// Optional multi-page configuration; [`super::Paging::none`] by default.
    paging: super::Paging,
}

/// Collapse runs of whitespace into single spaces and trim.
fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    let padding = bytes.iter().rev().take_while(|&&byte| byte == b'=').count();
    if padding > 2 || bytes[..bytes.len() - padding].contains(&b'=') {
        return None;
    }
    let data = &bytes[..bytes.len() - padding];
    if padding > 0 {
        if data.len() % 4 == 1 || padding != (4 - data.len() % 4) % 4 {
            return None;
        }
    } else if data.len() % 4 == 1 {
        return None;
    }

    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in data {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1u32 << bits) - 1;
        }
    }
    if bits > 0 && buffer != 0 {
        return None;
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
        let path_query = cfg.bool_param("path_query", false);
        let query_param = cfg.string_param("query_param", "q");
        let page_param = cfg.string_param("page_param", "").trim().to_owned();
        let page_in_path = cfg.bool_param("page_in_path", false);
        let paging = super::paging_from_config(
            name,
            cfg,
            (!page_param.is_empty()).then_some(page_param),
            page_in_path,
            &[query_param.as_str()],
        )?;
        let endpoint_url =
            super::validate_endpoint(name, &endpoint, path_query, paging.first_page())?;
        if !matches!(endpoint_url.scheme(), "http" | "https") {
            anyhow::bail!("engine '{name}': endpoint must use http or https");
        }
        // `query_pairs_mut().append_pair` below adds to the endpoint's query
        // string, so a key the endpoint already pins would ship twice
        // (`page=1&page=2`), the provider would honour the stale first value
        // and the loop would never leave page 1.
        super::reject_pinned_page_param(name, &endpoint_url, &paging)?;
        let result_selector = cfg.string_param("result_selector", "");
        if result_selector.is_empty() {
            anyhow::bail!("engine '{name}' requires a result_selector");
        }
        let link_selector = cfg.string_param("link_selector", "");
        if link_selector.is_empty() {
            anyhow::bail!("engine '{name}' requires a link_selector");
        }
        let title_selector = cfg.string_param("title_selector", "");
        let title_attribute = cfg.string_param("title_attr", "");
        let snippet_selector = cfg.string_param("snippet_selector", "");
        let extra_params = {
            let params: Vec<(String, String)> = cfg
                .params
                .iter()
                .filter_map(|(key, value)| {
                    let raw = key.strip_prefix("param_")?;
                    let value = value.as_str()?;
                    (!raw.is_empty() || !value.is_empty())
                        .then(|| (raw.to_string(), value.to_string()))
                })
                .collect();
            // The computed page value *replaces* a statically pinned
            // `param_<page_param>` entry (several shipped catalog files pin
            // `param_first`/`param_page` to "1"). Emitting both would send
            // `first=1&first=36`, and providers honour the first occurrence, so
            // the paging loop would never leave page 1.
            let mut filtered = match paging.param() {
                Some(page_param) => params
                    .into_iter()
                    .filter(|(key, _)| key != page_param)
                    .collect(),
                None => params,
            };
            filtered.sort();
            filtered
        };
        Ok(Self {
            name: name.into(),
            client: reqwest::Client::builder().user_agent(user_agent).build()?,
            endpoint,
            path_query,
            query_param,
            result_selector: Selector::parse(&result_selector)
                .map_err(|e| anyhow::anyhow!("engine '{name}': invalid result_selector: {e:?}"))?,
            link_selector: Selector::parse(&link_selector)
                .map_err(|e| anyhow::anyhow!("engine '{name}': invalid link_selector: {e:?}"))?,
            title_selector: (!title_selector.is_empty())
                .then(|| {
                    Selector::parse(&title_selector).map_err(|e| {
                        anyhow::anyhow!("engine '{name}': invalid title_selector: {e:?}")
                    })
                })
                .transpose()?,
            title_attribute: (!title_attribute.is_empty()).then_some(title_attribute),
            snippet_selector: (!snippet_selector.is_empty())
                .then(|| {
                    Selector::parse(&snippet_selector).map_err(|e| {
                        anyhow::anyhow!("engine '{name}': invalid snippet_selector: {e:?}")
                    })
                })
                .transpose()?,
            link_url_attr: cfg.string_param("link_url_attr", "href"),
            extra_params,
            paging,
        })
    }

    /// Build the request URL for a query and 1-based page, panicking on an
    /// unusable endpoint.
    ///
    /// Test convenience wrapper: request handling uses [`Self::request_url`],
    /// so only compiled when tests are built.
    #[cfg(test)]
    pub fn build_url(&self, query: &str) -> Url {
        self.request_url(query, 1)
            .expect("configured endpoint is a valid URL")
    }

    /// Build the request URL for a query and 1-based page, reporting an
    /// unusable endpoint instead of panicking.
    pub fn request_url(&self, query: &str, page: usize) -> anyhow::Result<Url> {
        let name = &self.name;
        let page = self.paging.page_for(page);
        let page_param = self.paging.param().filter(|_| page.is_some());
        let mut url = super::resolve_endpoint(&self.endpoint, self.path_query, query, page)
            .map_err(|error| anyhow::anyhow!("engine '{name}': invalid endpoint: {error}"))?;
        // `query_pairs_mut` on a query-less URL adds a bare trailing `?`, so
        // only touch the query string when there is something to append. A
        // `page_in_path` engine with no extra params appends nothing, and a
        // `page_param` on its own is enough to make appending worthwhile.
        if !self.path_query || !self.extra_params.is_empty() || page_param.is_some() {
            let mut pairs = url.query_pairs_mut();
            if !self.path_query {
                pairs.append_pair(&self.query_param, query);
            }
            for (key, value) in &self.extra_params {
                pairs.append_pair(key, value);
            }
            // The page value travels last, and stands in for the static
            // `param_<name>` entry that `from_config` dropped.
            if let (Some(page_param), Some(page)) = (page_param, page) {
                pairs.append_pair(page_param, &page.to_string());
            }
        }
        Ok(url)
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
            let title_element = self
                .title_selector
                .as_ref()
                .and_then(|selector| element.select(selector).next());
            let title = self
                .title_attribute
                .as_deref()
                .and_then(|attribute| {
                    title_element
                        .and_then(|title| title.value().attr(attribute))
                        .map(normalize_ws)
                        .filter(|title| !title.is_empty())
                        .or_else(|| {
                            link.value()
                                .attr(attribute)
                                .map(normalize_ws)
                                .filter(|title| !title.is_empty())
                        })
                })
                .or_else(|| {
                    title_element.map(|title| normalize_ws(&title.text().collect::<String>()))
                })
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| normalize_ws(&link.text().collect::<String>()));
            if title.is_empty() {
                continue;
            }
            let raw_href = link.value().attr(&self.link_url_attr).unwrap_or_default();
            let raw_href = raw_href.trim();
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

    /// Fetch and parse one page. `budget` is the caller's *remaining*
    /// whole-search budget, not a fresh per-request timeout.
    async fn fetch_page(
        &self,
        url: &Url,
        budget: Duration,
    ) -> EngineResult<Vec<(String, String, Option<String>)>> {
        let resp = self
            .client
            .get(url.clone())
            .timeout(budget)
            .send()
            .await?
            .error_for_status()?;

        let body = resp.text().await?;
        Ok(self.parse_html(url, &body))
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
        // `Engine::search` documents that `timeout` bounds the whole request +
        // parse, so a multi-page loop shares this one deadline instead of
        // multiplying the caller's budget.
        let deadline = Instant::now() + timeout;
        let mut results: Vec<SearchResult> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        // First failure, reported only when no page ever parsed: an empty page
        // or a challenge page is `EngineError::Parse` today and must stay that
        // error for page 1.
        let mut failure: Option<EngineError> = None;
        // True once any page parsed with at least one entry. Decided
        // independently of the final truncation, so `limit = 0` still returns
        // `Ok(vec![])` exactly as before.
        let mut parsed_any = false;

        // `max_pages()` is 1 when paging is off, so this loop is exactly the
        // pre-change single request for every existing configuration.
        for page_number in 1..=self.paging.max_pages() {
            let first = page_number == 1;
            // Load guardrail: never fetch another page once the running total
            // already satisfies `limit`. Page 1 is exempt because a single
            // request was always made, even for `limit = 0`.
            if page_number > 1 && results.len() >= limit {
                break;
            }
            // Deadline: this page gets only what is left of the caller's
            // budget, and a page that cannot finish is never started.
            let Some(budget) = super::request_budget(deadline, first) else {
                break;
            };

            let url = match self.request_url(query, page_number) {
                Ok(url) => url,
                // A page-1 URL that cannot be built is the pre-change error.
                // A later page ending the loop instead of failing the search
                // matches what a later *request* failure does below, so the two
                // adapters degrade identically. Unreachable while a page value
                // is always ASCII digits, but a page template that stopped
                // being trivial must not turn a paged search into a hard error.
                Err(_) if first => return Err(EngineError::Parse),
                Err(_) => break,
            };
            debug!(engine = self.name(), %url, page = page_number, "requesting");

            let page_results = match self.fetch_page(&url, budget).await {
                Ok(parsed) => parsed,
                Err(error) => {
                    failure.get_or_insert(error);
                    break;
                }
            };
            if page_results.is_empty() {
                // Either no hits, or the engine served a challenge/consent
                // page. We cannot tell them apart definitively; report a soft
                // failure and keep whatever earlier pages produced.
                failure.get_or_insert(EngineError::Parse);
                break;
            }
            parsed_any = true;
            // Deduplicate across pages, first occurrence wins: a provider that
            // repeats its last page when exhausted would otherwise hand back
            // results the caller already has. Page 1 is kept exactly as the
            // single-request code path produced it - duplicates included - so
            // default-off output is unchanged; the set is still seeded with its
            // URLs so a later page cannot repeat them.
            let before = results.len();
            for (title, result_url, snippet) in page_results {
                let fresh = seen.insert(super::dedupe_url_key(&result_url).to_owned());
                if fresh || first {
                    results.push(SearchResult::new(self.name(), title, result_url, snippet));
                }
            }
            if results.len() == before {
                break;
            }
        }

        // A multi-page fetch can never over-return.
        results.truncate(limit);
        if !parsed_any {
            return Err(failure.unwrap_or(EngineError::Parse));
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::resolve_endpoint;

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

    fn scrape_config(params: &[(&str, &str)]) -> EngineConfig {
        EngineConfig {
            engine_type: "html_scrape".into(),
            enabled: true,
            params: params
                .iter()
                .map(|(k, v)| ((*k).into(), toml::Value::String((*v).into())))
                .collect(),
        }
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
    fn rejects_invalid_endpoint_without_panicking() {
        let error = HtmlScrape::from_config(
            "broken",
            &EngineConfig {
                engine_type: "html_scrape".into(),
                enabled: true,
                params: [
                    ("endpoint".to_string(), toml::Value::from("not a URL")),
                    ("result_selector".to_string(), toml::Value::from(".result")),
                    ("link_selector".to_string(), toml::Value::from("a")),
                ]
                .into_iter()
                .collect(),
            },
            "test/1",
        )
        .err()
        .expect("invalid endpoint should be rejected")
        .to_string();
        assert!(error.contains("invalid endpoint"));
    }

    #[test]
    fn rejects_non_http_endpoint() {
        let error = HtmlScrape::from_config(
            "broken",
            &EngineConfig {
                engine_type: "html_scrape".into(),
                enabled: true,
                params: [
                    (
                        "endpoint".to_string(),
                        toml::Value::from("file:///etc/passwd"),
                    ),
                    ("result_selector".to_string(), toml::Value::from(".result")),
                    ("link_selector".to_string(), toml::Value::from("a")),
                ]
                .into_iter()
                .collect(),
            },
            "test/1",
        )
        .err()
        .expect("non-HTTP endpoint should be rejected")
        .to_string();
        assert!(error.contains("must use http or https"));
    }

    #[test]
    fn uses_title_selector_when_configured() {
        let mut params = std::collections::BTreeMap::new();
        for (key, value) in [
            ("endpoint", "https://www.bing.com/search"),
            ("query_param", "q"),
            ("result_selector", "li.b_algo"),
            ("link_selector", "h2 a"),
            ("title_selector", ".title"),
        ] {
            params.insert(key.to_string(), toml::Value::from(value));
        }
        let cfg = HtmlScrape::from_config(
            "bing",
            &EngineConfig {
                engine_type: "html_scrape".into(),
                enabled: true,
                params,
            },
            "searxng-rs/test",
        )
        .unwrap();
        let html = r#"<li class="b_algo"><h2><a href="https://example.test"><span class="title">Primary title</span> Extra text</a></h2></li>"#;
        let results = cfg.parse_html(&cfg.build_url("rust"), html);
        assert_eq!(results[0].0, "Primary title");
    }

    #[test]
    fn uses_link_title_attribute_when_configured() {
        let mut params = std::collections::BTreeMap::new();
        for (key, value) in [
            ("endpoint", "https://www.deviantart.com/search"),
            ("query_param", "q"),
            ("result_selector", "div[data-testid=\"content_row\"]"),
            ("link_selector", "a[href][aria-label]"),
            ("title_attr", "aria-label"),
        ] {
            params.insert(key.to_string(), toml::Value::from(value));
        }
        let engine = HtmlScrape::from_config(
            "deviantart",
            &EngineConfig {
                engine_type: "html_scrape".into(),
                enabled: false,
                params,
            },
            "searxng-rs/test",
        )
        .unwrap();
        let html = r#"<div data-testid="content_row"><a href="/art" aria-label="  A <em>bright</em> artwork  "><img data-testid="thumb"></a></div>"#;
        let results = engine.parse_html(&engine.build_url("rust"), html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "A <em>bright</em> artwork");
    }

    #[test]
    fn title_attribute_falls_back_when_selected_title_attribute_is_blank() {
        let mut params = std::collections::BTreeMap::new();
        for (key, value) in [
            ("endpoint", "https://example.test/search"),
            ("result_selector", ".result"),
            ("link_selector", "a"),
            ("title_selector", ".title"),
            ("title_attr", "aria-label"),
        ] {
            params.insert(key.to_string(), toml::Value::from(value));
        }
        let engine = HtmlScrape::from_config(
            "custom",
            &EngineConfig {
                engine_type: "html_scrape".into(),
                enabled: true,
                params,
            },
            "test/1",
        )
        .unwrap();
        let html = r#"
            <div class="result">
                <span class="title" aria-label="  ">ignored</span>
                <a href="/result" aria-label="Useful title">link text</a>
            </div>
            <div class="result">
                <span class="title">
                </span>
                <a href="/blank-title-text">Fallback link text</a>
            </div>
            <div class="result">
                <a href="/no-title-element" aria-label="Link attribute">link text</a>
            </div>
        "#;
        let results = engine.parse_html(&engine.build_url("rust"), html);
        assert_eq!(
            results
                .iter()
                .map(|result| result.0.as_str())
                .collect::<Vec<_>>(),
            ["Useful title", "Fallback link text", "Link attribute"]
        );
    }

    #[test]
    fn trims_whitespace_around_result_hrefs() {
        let engine = engine();
        let base = engine.build_url("rust");
        let html = r#"<li class="b_algo"><h2><a href="  /trimmed  ">Trimmed</a></h2></li>"#;
        let results = engine.parse_html(&base, html);
        assert_eq!(results[0].1, "https://www.bing.com/trimmed");
    }

    #[test]
    fn rejects_non_web_result_hrefs() {
        let engine = engine();
        let base = engine.build_url("rust");
        let html = r#"
            <li class="b_algo"><h2><a href="javascript:alert(1)">JavaScript</a></h2></li>
            <li class="b_algo"><h2><a href="data:text/html,unsafe">Data</a></h2></li>
            <li class="b_algo"><h2><a href="file:///etc/passwd">File</a></h2></li>
        "#;
        assert!(engine.parse_html(&base, html).is_empty());
    }

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

    #[test]
    fn rejects_noncanonical_base64url_data() {
        for input in [
            "YQ==junk", // data after padding
            "Y=Q=",     // padding in the middle
            "YWJj=",    // impossible padding length
            "YQ=",      // impossible padding count
            "YR",       // nonzero unused trailing bits
            "YQ==j",    // invalid trailing character
            "YQ==/",    // standard-base64 alphabet
        ] {
            assert!(decode_base64url(input).is_none(), "accepted {input}");
        }
        assert!(decode_base64url("YQ==").is_some());
        assert!(decode_base64url("YQ").is_some());
        assert_eq!(decode_base64url("YWJj"), Some(b"abc".to_vec()));
    }

    #[test]
    fn path_query_encodes_the_query_into_the_endpoint() {
        for (endpoint, query, expected) in [
            // Multi-word terms: a space becomes %20, not `+` or a segment.
            (
                "https://www.wordnik.com/words/{query}",
                "rust async",
                "https://www.wordnik.com/words/rust%20async",
            ),
            // An empty query yields an empty path segment, not a broken URL.
            (
                "https://www.wordnik.com/words/{query}",
                "",
                "https://www.wordnik.com/words/",
            ),
            // `/` cannot introduce a new path segment.
            ("https://x.test/{query}", "a/b", "https://x.test/a%2Fb"),
            // `?` and `#` cannot introduce a query string or a fragment.
            (
                "https://x.test/{query}",
                "a?b#c",
                "https://x.test/a%3Fb%23c",
            ),
            // Already-encoded input is escaped again, never double-decoded.
            (
                "https://x.test/{query}",
                "100%20off",
                "https://x.test/100%2520off",
            ),
            // `&` and `+` cannot forge a query parameter.
            (
                "https://x.test/{query}",
                "a&b+c",
                "https://x.test/a%26b%2Bc",
            ),
            // Non-ASCII becomes its UTF-8 percent-encoded bytes.
            (
                "https://x.test/{query}",
                "caf\u{e9}",
                "https://x.test/caf%C3%A9",
            ),
            // Every occurrence of the placeholder is substituted.
            (
                "https://x.test/{query}/{query}/1",
                "q",
                "https://x.test/q/q/1",
            ),
            // A pre-existing query string is preserved.
            (
                "https://wttr.in/{query}?format=j1",
                "new york",
                "https://wttr.in/new%20york?format=j1",
            ),
            // The upstream engines this unblocks.
            (
                "https://kickass.to/usearch/{query}/1/",
                "rust lang",
                "https://kickass.to/usearch/rust%20lang/1/",
            ),
            (
                "https://stocksnap.io/api/search-photos/{query}/relevance/desc/1",
                "blue sky",
                "https://stocksnap.io/api/search-photos/blue%20sky/relevance/desc/1",
            ),
            (
                "https://dictzone.com/en-fr-dictionary/{query}",
                "a b",
                "https://dictzone.com/en-fr-dictionary/a%20b",
            ),
            (
                "https://x.test/rest/api/search/{query}",
                "rust",
                "https://x.test/rest/api/search/rust",
            ),
            (
                "https://x.test/search/{query}/page/1",
                "rust",
                "https://x.test/search/rust/page/1",
            ),
        ] {
            let e = HtmlScrape::from_config(
                "api",
                &scrape_config(&[
                    ("endpoint", endpoint),
                    ("path_query", "true"),
                    ("result_selector", ".result"),
                    ("link_selector", "a"),
                ]),
                "test/1",
            )
            .unwrap();
            assert_eq!(
                e.build_url(query).as_str(),
                expected,
                "{endpoint} + {query}"
            );
        }
    }

    #[test]
    fn path_query_keeps_configured_extra_params() {
        let e = HtmlScrape::from_config(
            "api",
            &scrape_config(&[
                ("endpoint", "https://x.test/{query}"),
                ("path_query", "true"),
                ("param_cc", "tr"),
                ("result_selector", ".result"),
                ("link_selector", "a"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(
            e.build_url("rust async").as_str(),
            "https://x.test/rust%20async?cc=tr"
        );
        // Extra params append after any query string already in the endpoint.
        let e = HtmlScrape::from_config(
            "wttr",
            &scrape_config(&[
                ("endpoint", "https://wttr.in/{query}?format=j1"),
                ("path_query", "true"),
                ("param_cc", "tr"),
                ("result_selector", ".result"),
                ("link_selector", "a"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(
            e.build_url("new york").as_str(),
            "https://wttr.in/new%20york?format=j1&cc=tr"
        );
    }

    #[test]
    fn path_query_never_leaves_a_trailing_question_mark() {
        let e = HtmlScrape::from_config(
            "wordnik",
            &scrape_config(&[
                ("endpoint", "https://www.wordnik.com/words/{query}"),
                ("path_query", "true"),
                ("result_selector", ".result"),
                ("link_selector", "a"),
            ]),
            "test/1",
        )
        .unwrap();
        let url = e.build_url("rust");
        assert_eq!(url.as_str(), "https://www.wordnik.com/words/rust");
        assert_eq!(url.query(), None);
    }

    #[test]
    fn path_query_defaults_to_query_string_behaviour() {
        for path_query in [None, Some("false")] {
            let mut params = vec![
                ("endpoint", "https://x.test/search"),
                ("result_selector", ".result"),
                ("link_selector", "a"),
            ];
            if let Some(value) = path_query {
                params.push(("path_query", value));
            }
            let e = HtmlScrape::from_config("api", &scrape_config(&params), "test/1").unwrap();
            assert_eq!(
                e.build_url("rust async").as_str(),
                "https://x.test/search?q=rust+async",
                "{params:?}"
            );
        }
    }

    #[test]
    fn rejects_inconsistent_path_query_configuration() {
        for (params, expected) in [
            // Enabled without a placeholder.
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("path_query", "true"),
                ],
                "no {query} placeholder",
            ),
            // A placeholder without the flag would ship as a literal.
            (
                vec![("endpoint", "https://example.test/{query}")],
                "path_query is not enabled",
            ),
            (
                vec![
                    ("endpoint", "https://example.test/{query}"),
                    ("path_query", "false"),
                ],
                "path_query is not enabled",
            ),
            // The substituted form is what must parse ...
            (
                vec![("endpoint", "{query}"), ("path_query", "true")],
                "invalid endpoint",
            ),
            (
                vec![("endpoint", "not a URL {query}"), ("path_query", "true")],
                "invalid endpoint",
            ),
            // ... and what must carry an http(s) scheme.
            (
                vec![
                    ("endpoint", "{query}://example.test/s"),
                    ("path_query", "true"),
                ],
                "must use http or https",
            ),
        ] {
            let mut full = params.clone();
            full.push(("result_selector", ".result"));
            full.push(("link_selector", "a"));
            let error = HtmlScrape::from_config("api", &scrape_config(&full), "test/1")
                .err()
                .unwrap_or_else(|| panic!("{params:?} should be rejected"))
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn path_query_urls_resolve_relative_hrefs() {
        let e = HtmlScrape::from_config(
            "wordnik",
            &scrape_config(&[
                ("endpoint", "https://www.wordnik.com/words/{query}"),
                ("path_query", "true"),
                ("result_selector", ".result"),
                ("link_selector", "a"),
            ]),
            "test/1",
        )
        .unwrap();
        let base = e.build_url("rust async");
        let results = e.parse_html(
            &base,
            r#"<div class="result"><a href="/words/rust">Rust</a></div>"#,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "https://www.wordnik.com/words/rust");
    }

    // --- multi-page (paging) support -------------------------------------

    /// One canned HTTP response for the recording test server.
    struct Page {
        delay: Duration,
        status: u16,
        body: String,
    }

    impl Page {
        fn ok(body: impl Into<String>) -> Self {
            Self {
                delay: Duration::ZERO,
                status: 200,
                body: body.into(),
            }
        }
        fn slow(body: impl Into<String>, delay: Duration) -> Self {
            Self {
                delay,
                status: 200,
                body: body.into(),
            }
        }
        fn status(status: u16) -> Self {
            Self {
                delay: Duration::ZERO,
                status,
                body: String::new(),
            }
        }
    }

    /// Serve `pages` in order, one per connection, recording every request
    /// line.
    ///
    /// The task keeps accepting until the test aborts it, so a test that issues
    /// fewer requests than it queued can still read the recorded lines. A
    /// request past the end of the script is answered with a distinctive `503`
    /// and an empty body, which parses to zero results and therefore stops the
    /// loop instead of silently passing, and `Connection: close` keeps every
    /// request on its own connection so the client cannot reorder them.
    async fn recording_server(
        pages: Vec<Page>,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&recorded);
        let mut queue = std::collections::VecDeque::from(pages);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let page = queue.pop_front().unwrap_or(Page {
                    delay: Duration::ZERO,
                    status: 503,
                    body: "server-exhausted".into(),
                });
                let sink = std::sync::Arc::clone(&sink);
                tokio::spawn(async move {
                    let mut buffer = [0u8; 8192];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    sink.lock().unwrap().push(
                        String::from_utf8_lossy(&buffer[..read])
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .to_owned(),
                    );
                    if !page.delay.is_zero() {
                        tokio::time::sleep(page.delay).await;
                    }
                    let reason = if page.status == 200 { "OK" } else { "Error" };
                    let response = format!(
                        "HTTP/1.1 {} {}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        page.status,
                        reason,
                        page.body.len(),
                        page.body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (addr, recorded, task)
    }

    /// Stop the test server and return every request line it saw, in order.
    async fn request_lines(
        task: tokio::task::JoinHandle<()>,
        recorded: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Vec<String> {
        task.abort();
        recorded.lock().unwrap().clone()
    }

    /// A results page with `count` entries, titled `T<first>..`, href `/<i>..`.
    fn html_page(count: usize, first: usize) -> String {
        let items = (first..first + count)
            .map(|index| format!(r#"<div class="result"><a href="/{index}">T{index}</a></div>"#))
            .collect::<String>();
        format!("<html><body>{items}</body></html>")
    }

    /// A scrape engine pointed at a recording server, with `paging` params.
    async fn paged_engine(
        pages: Vec<Page>,
        endpoint_suffix: &str,
        paging: &[(&str, &str)],
    ) -> (
        HtmlScrape,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let (addr, recorded, task) = recording_server(pages).await;
        let endpoint = format!("{addr}{endpoint_suffix}");
        let mut params = vec![
            ("endpoint", endpoint.as_str()),
            ("result_selector", ".result"),
            ("link_selector", "a"),
        ];
        params.extend(paging.iter().copied());
        let engine = HtmlScrape::from_config("api", &scrape_config(&params), "test/1")
            .unwrap_or_else(|error| panic!("{paging:?}: {error}"));
        (engine, recorded, task)
    }

    #[tokio::test]
    async fn no_paging_keys_issue_exactly_one_request_with_an_identical_url() {
        // `max_pages` on its own, with no `page_param`, must stay inert.
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(5, 1)),
                Page::ok(html_page(5, 6)),
                Page::ok(html_page(5, 11)),
            ],
            "/search",
            &[("param_cc", "tr"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust async", 5, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(
            request_lines(task, &recorded).await,
            vec!["GET /search?q=rust+async&cc=tr HTTP/1.1".to_owned()]
        );
        // The page-1 list, unchanged and in order.
        assert_eq!(
            results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["T1", "T2", "T3", "T4", "T5"]
        );
    }

    #[test]
    fn paging_defaults_are_off_and_single_page() {
        let e = HtmlScrape::from_config(
            "api",
            &scrape_config(&[
                ("endpoint", "https://example.test/search"),
                ("result_selector", ".result"),
                ("link_selector", "a"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(e.paging.max_pages(), 1);
        assert_eq!(e.paging.page_for(1), None);
        assert_eq!(e.paging.page_for(7), None);
        assert_eq!(e.paging.first_page(), None);
        assert_eq!(e.paging.param(), None);
        // No page value ever reaches the URL, and the pre-change URL is
        // unchanged.
        for page in 1..=7 {
            assert_eq!(
                e.request_url("rust", page).unwrap().as_str(),
                "https://example.test/search?q=rust"
            );
        }
    }

    #[tokio::test]
    async fn first_page_satisfying_limit_issues_one_request() {
        for (limit, expected) in [(5, 5), (4, 4)] {
            let (engine, recorded, task) = paged_engine(
                vec![Page::ok(html_page(5, 1)), Page::ok(html_page(5, 6))],
                "/search",
                &[("page_param", "first"), ("max_pages", "3")],
            )
            .await;
            let results = engine
                .search("rust", limit, Duration::from_secs(5))
                .await
                .unwrap();
            let lines = request_lines(task, &recorded).await;
            assert_eq!(lines.len(), 1, "limit {limit}: {lines:?}");
            assert_eq!(lines[0], "GET /search?q=rust&first=1 HTTP/1.1");
            assert_eq!(results.len(), expected, "limit {limit}");
        }
    }

    #[tokio::test]
    async fn a_second_page_is_requested_only_while_the_total_is_below_limit() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(3, 1)),
                Page::ok(html_page(3, 4)),
                Page::ok(html_page(3, 7)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 5, Duration::from_secs(5))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[1], "GET /search?q=rust&page=2 HTTP/1.1");
        assert_eq!(results.len(), 5);
    }

    #[tokio::test]
    async fn a_page_that_parses_to_zero_results_stops_the_loop() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(3, 1)),
                // A challenge/consent page: nothing parses, so the loop stops
                // instead of burning the rest of the page budget.
                Page::ok("<html><body>Are you a robot?</body></html>"),
                Page::ok(html_page(3, 7)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 100, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 2);
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn a_page_of_unusable_entries_stops_the_loop() {
        // Entries without an href or a title are skipped, so a page made only
        // of those parses to nothing and must end the loop.
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(3, 1)),
                Page::ok(r#"<div class="result"><a>No href</a></div>"#),
                Page::ok(html_page(3, 7)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 100, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 2);
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn max_pages_caps_the_number_of_requests() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(1, 1)),
                Page::ok(html_page(1, 2)),
                Page::ok(html_page(1, 3)),
                // A fourth page is queued and must never be requested.
                Page::ok(html_page(1, 4)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 100, Duration::from_secs(5))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn max_pages_above_the_hard_ceiling_is_rejected() {
        for (value, expected) in [
            ("11", "max_pages must be between 1 and 10, got 11"),
            ("0", "max_pages must be between 1 and 10, got 0"),
        ] {
            let error = HtmlScrape::from_config(
                "api",
                &scrape_config(&[
                    ("endpoint", "https://example.test/search"),
                    ("result_selector", ".result"),
                    ("link_selector", "a"),
                    ("page_param", "page"),
                    ("max_pages", value),
                ]),
                "test/1",
            )
            .err()
            .unwrap_or_else(|| panic!("max_pages = {value} should be rejected"))
            .to_string();
            assert!(error.contains(expected), "{error}");
            // The key is rejected even when paging is off, so it is never
            // silently inert.
            let error = HtmlScrape::from_config(
                "api",
                &scrape_config(&[
                    ("endpoint", "https://example.test/search"),
                    ("result_selector", ".result"),
                    ("link_selector", "a"),
                    ("max_pages", value),
                ]),
                "test/1",
            )
            .err()
            .unwrap_or_else(|| panic!("max_pages = {value} should be rejected when paging is off"))
            .to_string();
            assert!(error.contains(expected), "{error}");
        }
        // The ceiling itself is accepted ...
        let e = HtmlScrape::from_config(
            "api",
            &scrape_config(&[
                ("endpoint", "https://example.test/search"),
                ("result_selector", ".result"),
                ("link_selector", "a"),
                ("page_param", "page"),
                ("max_pages", "10"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(e.paging.max_pages(), 10);
        // ... and an unreadable value falls back to the documented default
        // instead of failing the engine.
        let e = HtmlScrape::from_config(
            "api",
            &scrape_config(&[
                ("endpoint", "https://example.test/search"),
                ("result_selector", ".result"),
                ("link_selector", "a"),
                ("page_param", "page"),
                ("max_pages", "-1"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(e.paging.max_pages(), 3);
    }

    #[tokio::test]
    async fn page_start_and_page_step_drive_the_page_parameter() {
        for (param, start, step, expected) in [
            // bing_images: ?q=..&first=1, first = 1 + (n-1)*35
            ("first", "1", "35", ["first=1", "first=36", "first=71"]),
            // docker_hub: ?from=<n*10>
            ("from", "0", "10", ["from=0", "from=10", "from=20"]),
            // chefkoch: ?offset=<n*20>
            ("offset", "0", "20", ["offset=0", "offset=20", "offset=40"]),
            // pexels, solidtorrents: ?page=<n>
            ("page", "1", "1", ["page=1", "page=2", "page=3"]),
            // wikicommons: ?gsroffset=<n*10>
            (
                "gsroffset",
                "0",
                "10",
                ["gsroffset=0", "gsroffset=10", "gsroffset=20"],
            ),
        ] {
            let (engine, recorded, task) = paged_engine(
                vec![
                    Page::ok(html_page(1, 1)),
                    Page::ok(html_page(1, 2)),
                    Page::ok(html_page(1, 3)),
                ],
                "/search",
                &[
                    ("page_param", param),
                    ("page_start", start),
                    ("page_step", step),
                    ("max_pages", "3"),
                ],
            )
            .await;
            let results = engine
                .search("rust", 100, Duration::from_secs(5))
                .await
                .unwrap();
            let lines = request_lines(task, &recorded).await;
            assert_eq!(lines.len(), 3, "{param} {start} {step}: {lines:?}");
            for (line, want) in lines.iter().zip(expected) {
                assert!(line.contains(want), "{line} should carry {want}");
                assert_eq!(
                    line.matches(want).count(),
                    1,
                    "{line} should carry {want} exactly once"
                );
            }
            assert_eq!(results.len(), 3, "{param}");
        }
    }

    #[tokio::test]
    async fn deadline_exhaustion_stops_the_loop_and_returns_partial_results() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::slow(html_page(2, 1), Duration::from_millis(400)),
                Page::ok(html_page(2, 3)),
            ],
            "/search",
            &[("page_param", "offset"), ("max_pages", "3")],
        )
        .await;
        // 600 ms of budget, 400 ms spent on page 1: 200 ms is left, below the
        // 500 ms floor, so page 2 is never started.
        let results = engine
            .search("rust", 10, Duration::from_millis(600))
            .await
            .expect("page 1 results are still returned");
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn a_sub_floor_timeout_still_issues_exactly_one_request() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(1, 1)),
                Page::ok(html_page(1, 2)),
                Page::ok(html_page(1, 3)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 100, Duration::from_millis(200))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn an_exhausted_budget_never_fans_out() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(1, 1)),
                Page::ok(html_page(1, 2)),
                Page::ok(html_page(1, 3)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let _ = engine.search("rust", 100, Duration::ZERO).await;
        let lines = request_lines(task, &recorded).await;
        assert!(lines.len() <= 1, "{lines:?}");
    }

    #[tokio::test]
    async fn the_combined_list_is_truncated_to_limit() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(10, 1)),
                Page::ok(html_page(10, 11)),
                Page::ok(html_page(10, 21)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 25, Duration::from_secs(5))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(results.len(), 25);
        // Order is preserved across pages: the last kept entry is the 5th item
        // of page 3.
        assert_eq!(results[24].title, "T25");
    }

    #[tokio::test]
    async fn repeated_results_are_deduplicated_keeping_first_occurrence() {
        // Page 2 repeats page 1 and adds one; page 3 is disjoint.
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(3, 1)),
                Page::ok(html_page(4, 1)),
                Page::ok(html_page(1, 5)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 10, Duration::from_secs(5))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(
            results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["T1", "T2", "T3", "T4", "T5"]
        );

        // A provider that repeats its last page when exhausted (Bing) adds
        // nothing on page 2, which ends the loop.
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(3, 1)),
                Page::ok(html_page(3, 1)),
                Page::ok(html_page(1, 9)),
            ],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 10, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 2);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn the_page_value_replaces_a_statically_configured_param_entry() {
        // `bing_images` pins `param_first = "1"`; emitting that next to the
        // computed value would send `first=1&first=36`, and providers honour
        // the first occurrence, so the loop would never leave page 1.
        let e = HtmlScrape::from_config(
            "bing_images",
            &scrape_config(&[
                ("endpoint", "https://www.bing.com/images/async"),
                ("query_param", "q"),
                ("param_mmasync", "1"),
                ("param_first", "1"),
                ("param_count", "35"),
                ("result_selector", "ul.dgControl_list > li"),
                ("link_selector", ".imgpt .lnkw a"),
                ("page_param", "first"),
                ("page_start", "1"),
                ("page_step", "35"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        for (page, expected) in [(1, "first=1"), (2, "first=36"), (3, "first=71")] {
            let url = e.request_url("rust", page).unwrap();
            let query = url.query().unwrap();
            assert_eq!(
                query.matches(expected).count(),
                1,
                "{query} should carry {expected} exactly once"
            );
            // The other static params survive, still in sorted order.
            assert!(query.contains("count=35"), "{query}");
            assert!(query.contains("mmasync=1"), "{query}");
        }
        // Without `page_param` the pinned entry is left alone.
        let e = HtmlScrape::from_config(
            "bing_images",
            &scrape_config(&[
                ("endpoint", "https://www.bing.com/images/async"),
                ("param_mmasync", "1"),
                ("param_first", "1"),
                ("param_count", "35"),
                ("result_selector", "ul.dgControl_list > li"),
                ("link_selector", ".imgpt .lnkw a"),
            ]),
            "test/1",
        )
        .unwrap();
        assert!(
            e.request_url("rust", 1)
                .unwrap()
                .as_str()
                .contains("first=1")
        );
    }

    #[tokio::test]
    async fn the_page_value_replaces_a_statically_configured_param_on_the_wire() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(1, 1)),
                Page::ok(html_page(1, 36)),
                Page::ok(html_page(1, 71)),
            ],
            "/images/async",
            &[
                ("query_param", "q"),
                ("param_mmasync", "1"),
                ("param_first", "1"),
                ("param_count", "35"),
                ("page_param", "first"),
                ("page_start", "1"),
                ("page_step", "35"),
                ("max_pages", "3"),
            ],
        )
        .await;
        let results = engine
            .search("rust", 100, Duration::from_secs(5))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        for (line, first) in lines.iter().zip(["1", "36", "71"]) {
            assert_eq!(line.matches("first=").count(), 1, "{line}");
            assert!(line.contains(&format!("&first={first}")), "{line}");
            assert!(line.contains("mmasync=1"), "{line}");
            assert!(line.contains("count=35"), "{line}");
        }
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn the_shipped_bing_images_entry_pages_when_given_paging_keys() {
        // The catalog entry is out of scope for this change, but adding the
        // keys to it must work without any other edit.
        let mut cfg = crate::config::Config::builtin_defaults().engines["bing_images"].clone();
        cfg.params
            .insert("page_param".into(), toml::Value::from("first"));
        cfg.params.insert("page_start".into(), toml::Value::from(1));
        cfg.params.insert("page_step".into(), toml::Value::from(35));
        cfg.params.insert("max_pages".into(), toml::Value::from(3));
        let e = HtmlScrape::from_config("bing_images", &cfg, "test/1").unwrap();
        assert_eq!(e.paging.max_pages(), 3);
        let url = e.request_url("rust", 2).unwrap();
        let query = url.query().unwrap();
        assert_eq!(query.matches("first=").count(), 1, "{query}");
        assert!(query.contains("first=36"), "{query}");
    }

    #[tokio::test]
    async fn page_in_path_substitutes_decimal_digits_into_the_path() {
        for (suffix, start, step, expected) in [
            (
                "/usearch/{query}/{page}/",
                "1",
                "35",
                [
                    "/usearch/a%2Fb/1/",
                    "/usearch/a%2Fb/36/",
                    "/usearch/a%2Fb/71/",
                ],
            ),
            (
                "/search/{query}/page/{page}/",
                "1",
                "35",
                [
                    "/search/a%2Fb/page/1/",
                    "/search/a%2Fb/page/36/",
                    "/search/a%2Fb/page/71/",
                ],
            ),
            // An offset in a path segment: page 1 is 0, never -10.
            (
                "/{query}/{page}",
                "0",
                "10",
                ["/a%2Fb/0", "/a%2Fb/10", "/a%2Fb/20"],
            ),
        ] {
            let (engine, recorded, task) = paged_engine(
                vec![
                    Page::ok(html_page(1, 1)),
                    Page::ok(html_page(1, 2)),
                    Page::ok(html_page(1, 3)),
                ],
                suffix,
                &[
                    ("path_query", "true"),
                    ("page_in_path", "true"),
                    ("page_start", start),
                    ("page_step", step),
                    ("max_pages", "3"),
                ],
            )
            .await;
            let results = engine
                .search("a/b", 100, Duration::from_secs(5))
                .await
                .unwrap();
            let lines = request_lines(task, &recorded).await;
            assert_eq!(lines.len(), 3, "{suffix}: {lines:?}");
            for (line, want) in lines.iter().zip(expected) {
                assert!(line.starts_with(&format!("GET {want} HTTP/1.1")), "{line}");
            }
            assert_eq!(results.len(), 3, "{suffix}");
        }
    }

    #[test]
    fn a_page_value_can_never_inject_a_path_segment() {
        // The page value goes through the same unreserved-set encoding as the
        // search term, so it cannot add a segment, query or fragment even if a
        // future refactor made it string-sourced.
        for (template, query, page, expected) in [
            ("https://x.test/{page}", "q", Some(36), "https://x.test/36"),
            ("https://x.test/{page}", "q", Some(0), "https://x.test/0"),
            (
                "https://x.test/{query}/{page}",
                "a/b",
                Some(36),
                "https://x.test/a%2Fb/36",
            ),
            (
                "https://x.test/{page}?x=1",
                "q",
                Some(36),
                "https://x.test/36?x=1",
            ),
        ] {
            let url = resolve_endpoint(template, true, query, page).unwrap();
            assert_eq!(url.as_str(), expected, "{template}");
        }
        // An unsubstituted placeholder is silently rewritten to `%7Bpage%7D`,
        // which is exactly why a `{page}` endpoint must be rejected when
        // `page_in_path` is off.
        assert_eq!(
            resolve_endpoint("https://x.test/{page}", false, "q", None)
                .unwrap()
                .as_str(),
            "https://x.test/%7Bpage%7D"
        );
    }

    #[tokio::test]
    async fn page_in_path_never_leaves_a_trailing_question_mark() {
        let (engine, recorded, task) = paged_engine(
            vec![Page::ok(html_page(1, 1)), Page::ok(html_page(1, 36))],
            "/words/{query}/{page}",
            &[
                ("path_query", "true"),
                ("page_in_path", "true"),
                ("page_start", "1"),
                ("page_step", "35"),
                ("max_pages", "2"),
            ],
        )
        .await;
        let results = engine
            .search("rust", 100, Duration::from_secs(5))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        for line in &lines {
            assert!(!line.ends_with("? HTTP/1.1"), "{line}");
        }
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn rejects_inconsistent_paging_configuration() {
        for (params, expected) in [
            // `page_param` and `page_in_path` are mutually exclusive, and this
            // is reported before the missing `{page}` placeholder.
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("page_param", "first"),
                    ("page_in_path", "true"),
                ],
                "page_param and page_in_path are mutually exclusive",
            ),
            // Enabled without a placeholder.
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("page_in_path", "true"),
                ],
                "no {page} placeholder",
            ),
            // A placeholder without the flag would ship as a literal.
            (
                vec![("endpoint", "https://example.test/{page}")],
                "page_in_path is not enabled",
            ),
            (
                vec![
                    ("endpoint", "https://example.test/{page}"),
                    ("page_in_path", "false"),
                ],
                "page_in_path is not enabled",
            ),
            // The substituted form is what must parse ...
            (
                vec![("endpoint", "not a URL {page}"), ("page_in_path", "true")],
                "invalid endpoint",
            ),
            // ... and what must carry an http(s) scheme. A page value is always
            // digits, so the substituted scheme cannot come from `{page}`
            // itself: here it comes from the template around it.
            (
                vec![
                    ("endpoint", "mailto:{page}@example.test"),
                    ("page_in_path", "true"),
                ],
                "must use http or https",
            ),
        ] {
            let mut full = params.clone();
            full.push(("result_selector", ".result"));
            full.push(("link_selector", "a"));
            let error = HtmlScrape::from_config("api", &scrape_config(&full), "test/1")
                .err()
                .unwrap_or_else(|| panic!("{params:?} should be rejected"))
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn a_paging_misconfiguration_is_reported_before_a_missing_selector() {
        // Ordering guard: the endpoint checks run first, so a broken paging
        // configuration is what the operator is told about.
        let error = HtmlScrape::from_config(
            "api",
            &scrape_config(&[
                ("endpoint", "https://example.test/search"),
                ("page_in_path", "true"),
            ]),
            "test/1",
        )
        .err()
        .expect("the endpoint check should fire first")
        .to_string();
        assert!(error.contains("no {page} placeholder"), "{error}");
        assert!(!error.contains("result_selector"), "{error}");
    }

    #[test]
    fn page_param_must_not_collide_with_a_query_parameter() {
        for (params, expected) in [
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("page_param", "q"),
                ],
                Some("collides with a configured query or limit parameter"),
            ),
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("query_param", "search"),
                    ("page_param", "search"),
                ],
                Some("collides with a configured query or limit parameter"),
            ),
            // A static `param_<page_param>` entry is not a collision: the page
            // value replaces it.
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("param_page", "1"),
                    ("page_param", "page"),
                ],
                None,
            ),
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("page_param", "safe"),
                ],
                None,
            ),
        ] {
            let mut full = params.clone();
            full.push(("result_selector", ".result"));
            full.push(("link_selector", "a"));
            match expected {
                Some(expected) => {
                    let error = HtmlScrape::from_config("api", &scrape_config(&full), "test/1")
                        .err()
                        .unwrap_or_else(|| panic!("{params:?} should be rejected"))
                        .to_string();
                    assert!(error.contains(expected), "{error}");
                }
                None => {
                    HtmlScrape::from_config("api", &scrape_config(&full), "test/1")
                        .expect("a non-colliding page parameter is accepted");
                }
            }
        }
    }

    #[test]
    fn a_page_parameter_pinned_in_the_endpoint_is_rejected() {
        // `gitea` pins `page=1` in its endpoint; `request_url` appends the
        // computed value, which would ship `page=1&page=2`. Providers honour
        // the first occurrence, so the loop would silently re-request page 1
        // for the whole `max_pages` budget. A config error, like the
        // `param_<page>` duplicate that is removed instead.
        for (endpoint, page_param) in [
            (
                "https://gitea.com/api/v1/repos/search?sort=updated&order=desc&page=1",
                "page",
            ),
            (
                "https://api2.marginalia-search.com/search?page=1&nsfw=1",
                "page",
            ),
            ("https://example.test/search?offset=0", "offset"),
        ] {
            let error = HtmlScrape::from_config(
                "gitea",
                &scrape_config(&[
                    ("endpoint", endpoint),
                    ("result_selector", ".result"),
                    ("link_selector", "a"),
                    ("page_param", page_param),
                    ("max_pages", "3"),
                ]),
                "test/1",
            )
            .err()
            .unwrap_or_else(|| panic!("{endpoint} + page_param {page_param} should be rejected"))
            .to_string();
            // The message names the engine, the key and the endpoint.
            assert!(error.contains("engine 'gitea'"), "{error}");
            assert!(
                error.contains(&format!("page_param '{page_param}'")),
                "{error}"
            );
            assert!(
                error.contains("already present in the endpoint's query string"),
                "{error}"
            );
            assert!(error.contains(endpoint), "{error}");
        }

        // A pinned key that is not the page parameter is not a duplicate, and
        // a `page_in_path` engine appends no query key at all.
        for (endpoint, extra) in [
            (
                "https://example.test/search?nsfw=1",
                vec![("page_param", "page")],
            ),
            (
                "https://example.test/search/{page}?nsfw=1&page=1",
                vec![("page_in_path", "true")],
            ),
        ] {
            let mut params = vec![
                ("endpoint", endpoint),
                ("result_selector", ".result"),
                ("link_selector", "a"),
            ];
            params.extend(extra);
            HtmlScrape::from_config("api", &scrape_config(&params), "test/1")
                .unwrap_or_else(|error| panic!("{params:?}: {error}"));
        }

        // Paging off: a shipped entry that pins query keys still loads, and
        // keeps sending them.
        let config = crate::config::Config::builtin_defaults();
        let e = HtmlScrape::from_config("archlinux", &config.engines["archlinux"], "test/1")
            .expect("a shipped html_scrape entry with a pinned query string still loads");
        assert_eq!(
            e.request_url("rust lang", 1).unwrap().as_str(),
            "https://wiki.archlinux.de/index.php?search=rust+lang&limit=20&profile=default&title=Spezial%3ASuche"
        );
    }

    #[tokio::test]
    async fn a_pinned_key_that_is_not_the_page_parameter_still_pages() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(1, 1)),
                Page::ok(html_page(1, 2)),
                Page::ok(html_page(1, 3)),
            ],
            "/search?nsfw=1",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 100, Duration::from_secs(5))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        for (line, page) in lines.iter().zip(["1", "2", "3"]) {
            assert_eq!(line.matches("page=").count(), 1, "{line}");
            assert!(line.contains(&format!("&page={page} ")), "{line}");
        }
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn a_second_page_error_keeps_the_results_collected_so_far() {
        let (engine, recorded, task) = paged_engine(
            vec![Page::ok(html_page(2, 1)), Page::status(500)],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        let results = engine
            .search("rust", 10, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 2);
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn a_first_page_error_stays_strict_with_paging_on() {
        let (engine, recorded, task) = paged_engine(
            vec![Page::status(500)],
            "/search",
            &[("page_param", "page"), ("max_pages", "3")],
        )
        .await;
        assert!(matches!(
            engine.search("rust", 10, Duration::from_secs(5)).await,
            Err(EngineError::Http(_))
        ));
        assert_eq!(request_lines(task, &recorded).await.len(), 1);
    }

    #[tokio::test]
    async fn duplicates_within_the_first_page_are_kept_when_paging_is_off() {
        // Deduplication is for *cross-page* repeats. With paging off the
        // output must stay exactly what the single request produced, including
        // a provider that lists the same URL twice on one page.
        let (engine, recorded, task) = paged_engine(
            vec![Page::ok(
                r#"<div class="result"><a href="/1">First</a></div>
                   <div class="result"><a href="/1">Again</a></div>
                   <div class="result"><a href="/2">Second</a></div>"#,
            )],
            "/search",
            &[],
        )
        .await;
        let results = engine
            .search("rust", 10, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 1);
        assert_eq!(
            results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["First", "Again", "Second"]
        );
    }

    #[tokio::test]
    async fn page_value_arithmetic_saturates_instead_of_overflowing() {
        let (engine, recorded, task) = paged_engine(
            vec![
                Page::ok(html_page(1, 1)),
                Page::ok(html_page(1, 2)),
                Page::ok(html_page(1, 3)),
            ],
            "/search",
            &[
                ("page_param", "offset"),
                ("page_start", "18446744073709551615"),
                ("page_step", "18446744073709551615"),
                ("max_pages", "3"),
            ],
        )
        .await;
        let results = engine
            .search("rust", 100, Duration::from_secs(5))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(results.len(), 3);
        for line in &lines {
            assert!(line.contains("offset=18446744073709551615"), "{line}");
        }
    }

    #[test]
    fn paging_accepts_native_toml_booleans_and_integers() {
        let config = crate::config::Config::from_toml_str(
            r#"
[engines.api]
type = "html_scrape"
endpoint = "https://example.test/search"
result_selector = ".result"
link_selector = "a"
page_param = "offset"
page_start = 0
page_step = 20
max_pages = 4
"#,
        )
        .unwrap();
        let e = HtmlScrape::from_config("api", &config.engines["api"], "test/1").unwrap();
        assert_eq!(e.paging.param(), Some("offset"));
        assert_eq!(e.paging.max_pages(), 4);
        assert_eq!(e.paging.page_for(1), Some(0));
        assert_eq!(e.paging.page_for(3), Some(40));
        assert_eq!(e.paging.first_page(), None);
        assert_eq!(
            e.request_url("rust", 2).unwrap().as_str(),
            "https://example.test/search?q=rust&offset=20"
        );

        let config = crate::config::Config::from_toml_str(
            r#"
[engines.api]
type = "html_scrape"
endpoint = "https://example.test/search/{query}/{page}"
path_query = true
result_selector = ".result"
link_selector = "a"
page_in_path = true
page_start = 0
page_step = 10
max_pages = 4
"#,
        )
        .unwrap();
        let e = HtmlScrape::from_config("api", &config.engines["api"], "test/1").unwrap();
        assert_eq!(e.paging.first_page(), Some(0));
        assert_eq!(e.paging.param(), None);
        assert_eq!(
            e.request_url("rust", 1).unwrap().as_str(),
            "https://example.test/search/rust/0"
        );
    }
}
