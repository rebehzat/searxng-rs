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
    path_query: bool,
    query_param: String,
    extra_params: Vec<(String, String)>,
    result_selector: Selector,
    link_selector: Selector,
    title_selector: Option<Selector>,
    title_attribute: Option<String>,
    snippet_selector: Option<Selector>,
    link_url_attr: String,
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
        let endpoint_url = super::validate_endpoint(name, &endpoint, path_query)?;
        if !matches!(endpoint_url.scheme(), "http" | "https") {
            anyhow::bail!("engine '{name}': endpoint must use http or https");
        }
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
            path_query,
            query_param: cfg.string_param("query_param", "q"),
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
        })
    }

    /// Build the request URL for a query, panicking on an unusable endpoint.
    ///
    /// Test convenience wrapper: request handling uses
    /// [`Self::try_build_url`], so only compiled when tests are built.
    #[cfg(test)]
    pub fn build_url(&self, query: &str) -> Url {
        self.try_build_url(query)
            .expect("configured endpoint is a valid URL")
    }

    /// Build the request URL for a query, reporting an unusable endpoint
    /// instead of panicking.
    pub fn try_build_url(&self, query: &str) -> anyhow::Result<Url> {
        let name = &self.name;
        let mut url = super::resolve_endpoint(&self.endpoint, self.path_query, query)
            .map_err(|error| anyhow::anyhow!("engine '{name}': invalid endpoint: {error}"))?;
        // `query_pairs_mut` on a query-less URL adds a bare trailing `?`, so
        // only touch the query string when there is something to append.
        if !self.path_query || !self.extra_params.is_empty() {
            let mut pairs = url.query_pairs_mut();
            if !self.path_query {
                pairs.append_pair(&self.query_param, query);
            }
            for (key, value) in &self.extra_params {
                pairs.append_pair(key, value);
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
        // `from_config` already validated the substituted form, and the
        // substituted region holds only unreserved characters, so this cannot
        // fail at request time.
        let url = self.try_build_url(query).map_err(|_| EngineError::Parse)?;
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
}
