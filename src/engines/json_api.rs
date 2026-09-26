//! Configurable JSON adapter for documented provider APIs.
//!
//! Providers may expose results under a dotted path such as `message.items`;
//! result fields may also be dotted (`primary_location.landing_page_url`).
//! This adapter performs ordinary public or API-key-authenticated requests;
//! it never circumvents provider controls.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Url;
use scraper::ElementRef;
use serde_json::Value;

use crate::config::EngineConfig;
use crate::engines::Engine;
use crate::error::{EngineError, EngineResult};
use crate::models::SearchResult;

pub struct JsonApi {
    name: String,
    client: reqwest::Client,
    endpoint: String,
    path_query: bool,
    query_param: String,
    limit_param: Option<String>,
    results_path: Option<String>,
    title_field: String,
    url_field: String,
    url_prefix: String,
    url_template: Option<String>,
    url_source_field: Option<String>,
    fallback_url_field: Option<String>,
    fallback_url_template: Option<String>,
    max_limit: Option<usize>,
    /// Optional multi-page configuration; [`super::Paging::none`] by default.
    paging: super::Paging,
    snippet_field: String,
    array_value_field: Option<String>,
    normalize_title_html: bool,
    normalize_snippet_html: bool,
    snippet_max_length: Option<usize>,
    api_key: Option<(String, String)>,
}

impl JsonApi {
    pub fn from_config(name: &str, cfg: &EngineConfig, user_agent: &str) -> anyhow::Result<Self> {
        let endpoint = cfg.string_param("endpoint", "");
        if endpoint.is_empty() {
            return Err(anyhow::anyhow!("engine '{name}' requires an endpoint"));
        }
        let path_query = cfg.bool_param("path_query", false);
        let query_param = cfg.string_param("query_param", "q");
        let limit_param = cfg.string_param("limit_param", "").trim().to_owned();
        let page_param = cfg.string_param("page_param", "").trim().to_owned();
        let page_in_path = cfg.bool_param("page_in_path", false);
        let paging = super::paging_from_config(
            name,
            cfg,
            (!page_param.is_empty()).then_some(page_param),
            page_in_path,
            &[query_param.as_str(), limit_param.as_str()],
        )?;
        let endpoint_url =
            super::validate_endpoint(name, &endpoint, path_query, paging.first_page())?;
        if !matches!(endpoint_url.scheme(), "http" | "https") {
            return Err(anyhow::anyhow!(
                "engine '{name}': endpoint must use http or https"
            ));
        }
        // Both reqwest's `query()` and any hand-rolled append_pair() add to
        // the endpoint's query string, so a key the endpoint already pins
        // would ship twice and the loop would never leave page 1.
        super::reject_pinned_page_param(name, &endpoint_url, &paging)?;
        let api_key = match cfg.params.get("api_key_env") {
            Some(toml::Value::String(env_name)) if !env_name.is_empty() => std::env::var(env_name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (cfg.string_param("api_key_header", "Authorization"), value)),
            _ => None,
        };
        Ok(Self {
            name: name.into(),
            client: reqwest::Client::builder().user_agent(user_agent).build()?,
            endpoint,
            path_query,
            query_param,
            limit_param: (!limit_param.is_empty()).then_some(limit_param),
            results_path: (!cfg.string_param("results_path", "").is_empty())
                .then(|| cfg.string_param("results_path", "")),
            title_field: cfg.string_param("title_field", "title"),
            url_field: cfg.string_param("url_field", "url"),
            url_prefix: cfg.string_param("url_prefix", ""),
            url_template: (!cfg.string_param("url_template", "").is_empty())
                .then(|| cfg.string_param("url_template", "")),
            url_source_field: (!cfg.string_param("url_source_field", "").is_empty())
                .then(|| cfg.string_param("url_source_field", "")),
            fallback_url_field: (!cfg.string_param("fallback_url_field", "").is_empty())
                .then(|| cfg.string_param("fallback_url_field", "")),
            fallback_url_template: (!cfg.string_param("fallback_url_template", "").is_empty())
                .then(|| cfg.string_param("fallback_url_template", "")),
            max_limit: cfg.usize_param("max_limit", None),
            paging,
            snippet_field: cfg.string_param("snippet_field", "snippet"),
            array_value_field: (!cfg.string_param("array_value_field", "").is_empty())
                .then(|| cfg.string_param("array_value_field", "")),
            normalize_title_html: cfg.bool_param("normalize_title_html", false),
            normalize_snippet_html: cfg.bool_param("normalize_snippet_html", false),
            snippet_max_length: cfg.usize_param("snippet_max_length", None),
            api_key,
        })
    }

    fn has_result_array(&self, value: &Value) -> bool {
        if value.get("error").is_some_and(is_provider_error) {
            return false;
        }
        self.results_path
            .as_deref()
            .and_then(|path| value_at(value, path))
            .or_else(|| value.get("results"))
            .or_else(|| value.get("items"))
            .or(Some(value))
            .and_then(Value::as_array)
            .is_some()
    }

    fn parse_value_checked(&self, value: Value, limit: usize) -> EngineResult<Vec<SearchResult>> {
        if !self.has_result_array(&value) {
            return Err(EngineError::Parse);
        }
        Ok(self.parse_value(value, limit))
    }

    fn parse_value(&self, value: Value, limit: usize) -> Vec<SearchResult> {
        let candidates = self
            .results_path
            .as_deref()
            .and_then(|path| value_at(&value, path))
            .or_else(|| value.get("results"))
            .or_else(|| value.get("items"))
            .unwrap_or(&value);
        let Some(items) = candidates.as_array() else {
            return Vec::new();
        };
        items
            .iter()
            .take(limit)
            .filter_map(|item| {
                let mut title = self.text_at(item, &self.title_field)?;
                if self.normalize_title_html {
                    title = html_to_text(&title);
                }
                if title.trim().is_empty() {
                    return None;
                }

                let primary_url = self
                    .text_at(item, &self.url_field)
                    .map(|url| url.trim().to_owned())
                    .filter(|url| !url.is_empty());
                let candidate = primary_url
                    .as_deref()
                    .and_then(|url| self.build_result_url(item, url, true))
                    .or_else(|| self.fallback_url(item))?;
                // A present-but-unusable primary URL should not discard a valid
                // configured fallback (for example, an API's "N/A" sentinel).
                let url = normalized_web_url(&candidate).or_else(|| {
                    self.fallback_url(item)
                        .and_then(|fallback| normalized_web_url(&fallback))
                })?;

                let snippet = self
                    .text_at(item, &self.snippet_field)
                    .map(|snippet| {
                        let snippet = if self.normalize_snippet_html {
                            html_to_text(&snippet)
                        } else {
                            snippet
                        };
                        truncate_text(snippet, self.snippet_max_length)
                    })
                    .filter(|snippet| !snippet.trim().is_empty());
                Some(SearchResult::with_metadata(
                    self.name.clone(),
                    title,
                    url,
                    snippet,
                    HashMap::new(),
                ))
            })
            .collect()
    }

    fn text_at(&self, value: &Value, path: &str) -> Option<String> {
        let value = value_at(value, path)?;
        if let Some(text) = scalar_text(value) {
            return Some(text);
        }
        let items = value.as_array()?;
        if let Some(array_value_field) = &self.array_value_field {
            let value = items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .or_else(|| scalar_text(value_at(item, array_value_field)?))
                })
                .collect::<String>();
            let value = normalize_ws(&value);
            (!value.is_empty()).then_some(value)
        } else {
            // Preserve the original string-array behavior when object-array
            // extraction was not requested.
            items.iter().find_map(Value::as_str).map(str::to_owned)
        }
    }

    fn build_result_url(&self, item: &Value, value: &str, primary: bool) -> Option<String> {
        let mut url = value.to_owned();
        if primary && let Some(template) = &self.url_template {
            let source = self
                .url_source_field
                .as_deref()
                .and_then(|field| self.text_at(item, field))
                .unwrap_or_default();
            url = template
                .replace("{value}", &url)
                .replace("{url}", &url)
                .replace("{source}", &source);
        }
        if !self.url_prefix.is_empty() {
            if let Ok(parsed) = Url::parse(&url) {
                if !matches!(parsed.scheme(), "http" | "https") {
                    return None;
                }
            } else {
                url = format!(
                    "{}/{}",
                    self.url_prefix.trim_end_matches('/'),
                    url.trim_start_matches('/')
                );
            }
        }
        Some(url)
    }

    fn fallback_url(&self, item: &Value) -> Option<String> {
        let field = self.fallback_url_field.as_deref()?;
        let value = self.text_at(item, field)?;
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        let fallback = if normalized_web_url(value).is_some() {
            // Some providers (for example OpenAlex) return a DOI as an
            // absolute URL, while others return the bare DOI. Avoid wrapping
            // an already usable URL in a fallback template.
            value.to_owned()
        } else {
            self.fallback_url_template.as_deref().map_or_else(
                || value.to_owned(),
                |template| template.replace("{value}", value).replace("{url}", value),
            )
        };
        self.build_result_url(item, &fallback, false)
    }

    /// Number of items a single page may hold: what this engine asks the
    /// provider for and what it parses out of one response.
    ///
    /// `max_limit` caps the *page size*, never the search total. Conflating the
    /// two is what would make paging dead for every `max_limit` engine: the
    /// running total would be compared against the clamped page size, the load
    /// guardrail would trip on page 1 and a second request would never be
    /// issued.
    fn page_size(&self, limit: usize) -> usize {
        self.max_limit.map_or(limit, |max| limit.min(max))
    }

    /// Fetch and decode one page. `page_size` is the provider-capped number of
    /// items requested and parsed for this page; `budget` is the caller's
    /// *remaining* whole-search budget, not a fresh per-request timeout.
    async fn fetch_page(
        &self,
        query: &str,
        page_size: usize,
        page: Option<usize>,
        budget: Duration,
    ) -> EngineResult<Value> {
        let params = self.request_params(query, page_size, page);
        // `from_config` already validated the substituted form, and the
        // substituted region holds only unreserved characters, so this cannot
        // fail at request time.
        let endpoint = super::resolve_endpoint(&self.endpoint, self.path_query, query, page)
            .map_err(|_| EngineError::Parse)?;
        let mut request = self.client.get(endpoint).query(&params).timeout(budget);
        if let Some((header, value)) = &self.api_key {
            request = request.header(header, value);
        }
        let body = request.send().await?.error_for_status()?.text().await?;
        serde_json::from_str(&body).map_err(|_| EngineError::Parse)
    }

    fn request_params(
        &self,
        query: &str,
        limit: usize,
        page: Option<usize>,
    ) -> Vec<(String, String)> {
        let limit = self.page_size(limit);
        let mut params = Vec::new();
        // With `path_query` the term travels in the URL path, never as a
        // query-string parameter.
        if !self.path_query {
            params.push((self.query_param.clone(), query.to_owned()));
        }
        if let Some(limit_param) = &self.limit_param {
            params.push((limit_param.clone(), limit.to_string()));
        }
        // The page value goes last, so a provider that ignores unknown keys
        // still sees the documented `?q=..&size=..&first=..` order.
        if let (Some(page_param), Some(page)) = (&self.paging.param, page) {
            params.push((page_param.clone(), page.to_string()));
        }
        params
    }
}

#[async_trait]
impl Engine for JsonApi {
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
        // `max_limit` clamps the per-request page size exactly as it always
        // did. The loop's target stays the caller's `limit`: the running total
        // is compared against it and the combined list is truncated to it, so
        // a paged engine can accumulate up to the caller's `limit` even when
        // every individual page is capped well below it.
        let page_size = self.page_size(limit);
        let mut collected: Vec<SearchResult> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // `max_pages()` is 1 when paging is off, so this loop is exactly the
        // pre-change single request for every existing configuration.
        for page_number in 1..=self.paging.max_pages() {
            let first = page_number == 1;
            // Load guardrail: never fetch another page once the running total
            // already satisfies `limit`. Page 1 is exempt because a single
            // request was always made, even for `limit = 0`.
            if page_number > 1 && collected.len() >= limit {
                break;
            }
            // `None` when paging is off: no page value, no substitution.
            let page = self.paging.page_for(page_number);
            // Deadline: this page gets only what is left of the caller's
            // budget, and a page that cannot finish is never started.
            let Some(budget) = super::request_budget(deadline, first) else {
                break;
            };

            let value = match self.fetch_page(query, page_size, page, budget).await {
                Ok(value) => value,
                // The first page stays strict, so with paging off this is the
                // pre-change error taxonomy.
                Err(error) if first => return Err(error),
                // A later page that fails ends the loop: the caller keeps what
                // earlier pages produced.
                Err(_) => break,
            };

            // Page 1 keeps the strict check that turns an error-shaped payload
            // into `EngineError::Parse`. A later page that lost its result
            // array ends the loop instead of failing the search.
            let page_results = if first {
                self.parse_value_checked(value, page_size)?
            } else if self.has_result_array(&value) {
                // Parsed at the full page size, never at the remaining need:
                // `parse_value` applies `take(limit)` before dropping unusable
                // entries, so a small remainder could turn a healthy page into
                // a false "empty page". Truncation happens once, at the end.
                self.parse_value(value, page_size)
            } else {
                Vec::new()
            };

            // Deduplicate across pages, first occurrence wins. Page 1 is kept
            // exactly as the single-request code path produced it - duplicates
            // included - so default-off output is unchanged; the set is still
            // seeded with its URLs so a later page cannot repeat them.
            let before = collected.len();
            for result in page_results {
                let fresh = seen.insert(super::dedupe_url_key(&result.url).to_owned());
                if fresh || first {
                    collected.push(result);
                }
            }
            // A page that adds nothing - empty, or a provider that repeats the
            // last page when exhausted (Bing) - ends the loop.
            if collected.len() == before {
                break;
            }
        }
        // A multi-page fetch can never over-return.
        collected.truncate(limit);
        Ok(collected)
    }
}

fn is_provider_error(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        // Numeric zero is a conventional success code in otherwise
        // error-shaped response envelopes.
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(error) => !error.trim().is_empty(),
        _ => true,
    }
}

fn normalized_web_url(candidate: &str) -> Option<String> {
    let url = Url::parse(candidate).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn value_at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .filter(|part| !part.is_empty())
        .try_fold(value, |current, part| current.get(part))
}

fn scalar_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_number().map(ToString::to_string))
}

fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_to_text(input: &str) -> String {
    fn is_block(name: &str) -> bool {
        matches!(
            name,
            "address"
                | "article"
                | "aside"
                | "blockquote"
                | "br"
                | "dd"
                | "details"
                | "dialog"
                | "div"
                | "dl"
                | "dt"
                | "fieldset"
                | "figcaption"
                | "figure"
                | "footer"
                | "form"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "header"
                | "hgroup"
                | "hr"
                | "li"
                | "main"
                | "nav"
                | "ol"
                | "p"
                | "pre"
                | "section"
                | "table"
                | "td"
                | "th"
                | "tr"
                | "ul"
        )
    }

    enum Pending<'a> {
        Element(ElementRef<'a>),
        Text(&'a str),
        Separator,
    }

    let fragment = scraper::Html::parse_fragment(input);
    let root = fragment.root_element();
    let mut pending = root
        .children()
        .rev()
        .filter_map(|node| {
            if let Some(text) = node.value().as_text() {
                Some(Pending::Text(text))
            } else {
                ElementRef::wrap(node).map(Pending::Element)
            }
        })
        .collect::<Vec<_>>();
    let mut text = String::new();
    while let Some(node) = pending.pop() {
        match node {
            Pending::Text(value) => text.push_str(value),
            Pending::Separator => text.push(' '),
            Pending::Element(element) => {
                let name = element.value().name();
                let hidden_attribute = element.value().attr("hidden").is_some()
                    || element
                        .value()
                        .attr("aria-hidden")
                        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
                if matches!(name, "script" | "style" | "template" | "noscript") || hidden_attribute
                {
                    continue;
                }
                if is_block(name) {
                    text.push(' ');
                    pending.push(Pending::Separator);
                }
                pending.extend(element.children().rev().filter_map(|child| {
                    if let Some(text) = child.value().as_text() {
                        Some(Pending::Text(text))
                    } else {
                        ElementRef::wrap(child).map(Pending::Element)
                    }
                }));
            }
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_text(text: String, max_length: Option<usize>) -> String {
    const ELLIPSIS: &str = "...";

    let Some(max_length) = max_length else {
        return text;
    };
    if text.chars().count() <= max_length {
        return text;
    }
    if max_length == 0 {
        return String::new();
    }

    let content_length = max_length.saturating_sub(ELLIPSIS.chars().count());
    let mut truncated = text.chars().take(content_length).collect::<String>();
    truncated.push_str(&ELLIPSIS.chars().take(max_length).collect::<String>());
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::resolve_endpoint;

    fn config(params: &[(&str, &str)]) -> EngineConfig {
        EngineConfig {
            engine_type: "json_api".into(),
            enabled: true,
            params: params
                .iter()
                .map(|(k, v)| ((*k).into(), toml::Value::String((*v).into())))
                .collect(),
        }
    }

    #[test]
    fn parses_nested_results_and_fields() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("results_path", "message.items"),
                ("title_field", "title"),
                ("url_field", "links.html"),
            ]),
            "test/1",
        )
        .unwrap();
        let value = serde_json::json!({"message":{"items":[{"title":["Rust"],"links":{"html":"https://rust-lang.org"}}]}});
        let results = e.parse_value(value, 5);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].url, "https://rust-lang.org/");
    }

    #[test]
    fn applies_url_prefix() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("url_prefix", "https://example.test"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(serde_json::json!({"results":[{"title":"A","url":"/a"}]}), 1);
        assert_eq!(results[0].url, "https://example.test/a");
    }

    #[test]
    fn applies_url_prefix_to_bare_identifiers() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("url_prefix", "https://archive.org/details"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{"title":"A","url":"item-1"}]}),
            1,
        );
        assert_eq!(results[0].url, "https://archive.org/details/item-1");
    }

    #[test]
    fn applies_url_template_to_bare_identifiers() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("url_template", "https://europepmc.org/article/{value}"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{"title":"A","url":"MED/123"}]}),
            1,
        );
        assert_eq!(results[0].url, "https://europepmc.org/article/MED/123");
    }

    #[test]
    fn applies_url_template_with_a_source_field() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                (
                    "url_template",
                    "https://europepmc.org/article/{source}/{value}",
                ),
                ("url_source_field", "source"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{"title":"A","source":"MED","url":"123"}]}),
            1,
        );
        assert_eq!(results[0].url, "https://europepmc.org/article/MED/123");
    }

    #[test]
    fn uses_fallback_url_when_primary_url_is_missing_or_empty() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("fallback_url_field", "objectID"),
                (
                    "fallback_url_template",
                    "https://news.ycombinator.com/item?id={value}",
                ),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[
                {"title":"Missing URL","objectID":"41","url":null},
                {"title":"Whitespace URL","objectID":42,"url":"   "},
                {"title":"Empty fallback","objectID":"  ","url":""}
            ]}),
            3,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://news.ycombinator.com/item?id=41");
        assert_eq!(results[1].url, "https://news.ycombinator.com/item?id=42");
    }

    #[test]
    fn fallback_url_is_not_reprocessed_by_primary_url_template() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("url_template", "https://example.test/primary/{value}"),
                ("fallback_url_field", "objectID"),
                (
                    "fallback_url_template",
                    "https://news.ycombinator.com/item?id={value}",
                ),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[
                {"title":"Fallback","objectID":"42","url":null}
            ]}),
            1,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://news.ycombinator.com/item?id=42");
    }

    #[test]
    fn absolute_fallback_url_bypasses_a_bare_identifier_template() {
        let config = crate::config::Config::builtin_defaults();
        let e = JsonApi::from_config("openalex", &config.engines["openalex"], "test/1").unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[
                {
                    "title":"Absolute DOI",
                    "url":null,
                    "doi":"https://doi.org/10.1234/absolute"
                },
                {
                    "title":"Bare DOI",
                    "url":null,
                    "doi":"10.1234/bare"
                }
            ]}),
            2,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://doi.org/10.1234/absolute");
        assert_eq!(results[1].url, "https://doi.org/10.1234/bare");
    }

    #[test]
    fn fallback_url_field_can_stand_alone_and_still_rejects_non_web_urls() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("fallback_url_field", "fallback"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[
                {"title":"Web fallback", "url":null, "fallback":"https://example.test/fallback"},
                {"title":"Unsafe fallback", "url":null, "fallback":"javascript:alert(1)"}
            ]}),
            2,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.test/fallback");
    }

    #[test]
    fn uses_fallback_url_when_primary_url_is_invalid() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("fallback_url_field", "objectID"),
                (
                    "fallback_url_template",
                    "https://news.ycombinator.com/item?id={value}",
                ),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[
                {"title":"Invalid primary","objectID":"43","url":"javascript:alert(1)"}
            ]}),
            1,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://news.ycombinator.com/item?id=43");
    }

    #[test]
    fn preserves_legacy_string_array_extraction_without_object_configuration() {
        let e = JsonApi::from_config(
            "api",
            &config(&[("endpoint", "https://example.test")]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{
                "title":["first","second"],
                "url":"https://example.test/article"
            }]}),
            1,
        );
        assert_eq!(results[0].title, "first");
    }

    #[test]
    fn joins_configured_object_array_values() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("array_value_field", "value"),
                ("snippet_field", "extract"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{
                "title":[{"value":"Rust"},{"value":" search"}],
                "url":"https://example.test/rust",
                "extract":["A fast ",{"value":"language"}]
            }]}),
            1,
        );
        assert_eq!(results[0].title, "Rust search");
        assert_eq!(results[0].snippet.as_deref(), Some("A fast language"));
    }

    #[test]
    fn joins_adjacent_object_array_fragments_without_inserting_spaces() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("array_value_field", "value"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{
                "title":[{"value":"R"},{"value":"u"},{"value":"s"},{"value":"t"}],
                "url":"https://example.test/rust"
            }]}),
            1,
        );
        assert_eq!(results[0].title, "Rust");
    }

    #[test]
    fn normalizes_whitespace_in_configured_object_array_fragments() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("array_value_field", "value"),
                ("snippet_field", "extract"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{
                "title":[{"value":" Rust\n documentation "}],
                "url":"https://example.test/rust",
                "extract":[{"value":"A fast\nlanguage"}]
            }]}),
            1,
        );
        assert_eq!(results[0].title, "Rust documentation");
        assert_eq!(results[0].snippet.as_deref(), Some("A fast language"));
    }

    #[test]
    fn configured_object_array_extraction_skips_malformed_fragments() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("array_value_field", "value"),
                ("snippet_field", "extract"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{
                "title":[
                    null,
                    {"other":"ignored"},
                    {"value":{"value":"ignored"}},
                    {"value":" Rust\n"},
                    " documentation "
                ],
                "url":"https://example.test/rust",
                "extract":[{"value":null},{"value":"Safe "},{"value":"text"}]
            }]}),
            1,
        );
        assert_eq!(results[0].title, "Rust documentation");
        assert_eq!(results[0].snippet.as_deref(), Some("Safe text"));
    }

    #[test]
    fn normalizes_configured_html_to_visible_text() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("normalize_title_html", "true"),
                ("normalize_snippet_html", "true"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{
                "title":"A (<i>quoted</i>) title",
                "url":"https://example.test/article",
                "snippet":"<p>Hello <strong>world</strong>.</p><script>tracking()</script><style>.hidden {}</style><span hidden>internal</span><span aria-hidden=\"true\">icon</span><p>Again</p>"
            }]}),
            1,
        );
        assert_eq!(results[0].title, "A (quoted) title");
        assert_eq!(results[0].snippet.as_deref(), Some("Hello world. Again"));
    }

    #[test]
    fn html_normalization_preserves_plain_text_and_decodes_entities() {
        assert_eq!(html_to_text("1 < 2 &amp; 3 > 2"), "1 < 2 & 3 > 2");
    }

    #[test]
    fn truncates_normalized_html_snippet_by_character() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("normalize_snippet_html", "true"),
                ("snippet_max_length", "5"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{
                "title":"HTML snippet",
                "url":"https://example.test/article",
                "snippet":"<p>αβγδεζη</p>"
            }]}),
            1,
        );
        assert_eq!(results[0].snippet.as_deref(), Some("αβ..."));
        assert_eq!(results[0].snippet.as_deref().unwrap().chars().count(), 5);
    }

    #[test]
    fn truncates_plain_snippets_when_max_length_is_configured() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("snippet_max_length", "5"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[{
                "title":"Plain snippet",
                "url":"https://example.test/article",
                "snippet":"abcdefgh"
            }]}),
            1,
        );
        assert_eq!(results[0].snippet.as_deref(), Some("ab..."));
    }

    #[test]
    fn sends_configured_limit_parameter() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("query_param", "search"),
                ("limit_param", "limit"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(
            e.request_params("rust async", 7, None),
            vec![
                ("search".to_string(), "rust async".to_string()),
                ("limit".to_string(), "7".to_string()),
            ]
        );
    }

    #[test]
    fn provider_max_limit_is_applied() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("limit_param", "per_page"),
                ("max_limit", "40"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(e.max_limit, Some(40));
        assert_eq!(
            e.request_params("rust", 50, None),
            vec![
                ("q".to_string(), "rust".to_string()),
                ("per_page".to_string(), "40".to_string()),
            ]
        );
    }

    #[test]
    fn rejects_non_web_result_urls() {
        let e = JsonApi::from_config(
            "api",
            &config(&[("endpoint", "https://example.test")]),
            "test/1",
        )
        .unwrap();
        for url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
        ] {
            let value = serde_json::json!({"results":[{"title":"unsafe","url":url}]});
            assert!(e.parse_value(value, 5).is_empty(), "accepted {url}");
        }
    }

    #[test]
    fn missing_endpoint_is_rejected() {
        assert!(JsonApi::from_config("api", &config(&[]), "test/1").is_err());
    }

    #[test]
    fn rejects_invalid_and_non_http_endpoints() {
        for (endpoint, expected) in [
            ("not a URL", "invalid endpoint"),
            ("file:///etc/passwd", "must use http or https"),
            ("https://", "invalid endpoint"),
        ] {
            let error =
                match JsonApi::from_config("api", &config(&[("endpoint", endpoint)]), "test/1") {
                    Ok(_) => panic!("invalid endpoint should be rejected"),
                    Err(error) => error.to_string(),
                };
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn accepts_native_toml_boolean_and_integer_options() {
        let config = crate::config::Config::from_toml_str(
            r#"
[engines.api]
type = "json_api"
endpoint = "https://example.test"
normalize_snippet_html = true
snippet_max_length = 5
max_limit = 40
"#,
        )
        .unwrap();
        let e = JsonApi::from_config("api", &config.engines["api"], "test/1").unwrap();
        assert!(e.normalize_snippet_html);
        assert_eq!(e.snippet_max_length, Some(5));
        assert_eq!(e.max_limit, Some(40));
    }

    #[test]
    fn rejects_blank_titles_and_snippets() {
        let e = JsonApi::from_config(
            "api",
            &config(&[("endpoint", "https://example.test")]),
            "test/1",
        )
        .unwrap();
        let results = e.parse_value(
            serde_json::json!({"results":[
                {"title":" \n\t", "url":"https://example.test/blank-title"},
                {"title":"Valid", "url":"https://example.test/valid", "snippet":"  \n"},
                {"title":"Also valid", "url":"https://example.test/valid-2", "snippet":"text"}
            ]}),
            3,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Valid");
        assert_eq!(results[0].snippet, None);
        assert_eq!(results[1].snippet.as_deref(), Some("text"));
    }

    #[test]
    fn rejects_error_shaped_payloads_but_accepts_empty_result_arrays() {
        let e = JsonApi::from_config(
            "api",
            &config(&[("endpoint", "https://example.test")]),
            "test/1",
        )
        .unwrap();
        for value in [
            serde_json::json!({"error":"rate limited"}),
            serde_json::json!({"error":"rate limited", "results":[]}),
            serde_json::json!({}),
            serde_json::Value::Null,
        ] {
            assert!(e.parse_value_checked(value, 5).is_err());
        }
        assert!(
            e.parse_value_checked(serde_json::json!({"results":[]}), 5)
                .is_ok()
        );
    }

    #[test]
    fn accepts_falsy_error_indicators_on_otherwise_valid_payloads() {
        let e = JsonApi::from_config(
            "api",
            &config(&[("endpoint", "https://example.test")]),
            "test/1",
        )
        .unwrap();
        for error in [
            serde_json::Value::Null,
            serde_json::Value::Bool(false),
            serde_json::json!(0),
            serde_json::json!(0.0),
            serde_json::json!(""),
            serde_json::json!(" \n\t"),
        ] {
            let value = serde_json::json!({
                "error": error,
                "results": [{
                    "title": "Valid",
                    "url": "https://example.test/result"
                }]
            });
            let results = e
                .parse_value_checked(value, 5)
                .expect("empty error indicator should not fail a valid response");
            assert_eq!(results[0].title, "Valid");
        }
    }

    #[test]
    fn validates_the_selected_result_array_shape() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test"),
                ("results_path", "message.items"),
            ]),
            "test/1",
        )
        .unwrap();
        let result = serde_json::json!({
            "title": "Valid",
            "url": "https://example.test/result"
        });
        assert!(
            e.parse_value_checked(
                serde_json::json!({
                    "message": {"items": {"unexpected": result.clone()}},
                    "results": [result.clone()]
                }),
                5,
            )
            .is_err()
        );
        assert!(
            e.parse_value_checked(serde_json::json!({"message": {"items": []}}), 5)
                .is_ok()
        );
        // Keep the legacy conventional-array fallback when a configured path
        // is absent from an otherwise compatible response.
        assert!(
            e.parse_value_checked(serde_json::json!({"results": [result]}), 5)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn search_reports_provider_error_payloads_as_parse_errors() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/search", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).await.unwrap();
            assert!(read > 0);
            let body = r#"{"error":"rate limited"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let e = JsonApi::from_config("api", &config(&[("endpoint", &endpoint)]), "test/1").unwrap();
        assert!(matches!(
            e.search("rust", 5, std::time::Duration::from_secs(1)).await,
            Err(EngineError::Parse)
        ));
        server.await.unwrap();
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
            let url = resolve_endpoint(endpoint, true, query, None).unwrap();
            assert_eq!(url.as_str(), expected, "{endpoint} + {query}");
            let e = JsonApi::from_config(
                "api",
                &config(&[("endpoint", endpoint), ("path_query", "true")]),
                "test/1",
            )
            .unwrap();
            assert!(e.path_query, "{endpoint}");
            let url = resolve_endpoint(&e.endpoint, e.path_query, query, None).unwrap();
            assert_eq!(url.as_str(), expected, "from_config wiring: {endpoint}");
            // The term never also travels as a query-string parameter.
            assert!(e.request_params(query, 5, None).is_empty(), "{endpoint}");
        }
    }

    #[test]
    fn path_query_keeps_the_limit_parameter() {
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://x.test/{query}"),
                ("path_query", "true"),
                ("limit_param", "n"),
                ("max_limit", "40"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(
            e.request_params("q", 50, None),
            vec![("n".to_string(), "40".to_string())]
        );
    }

    #[test]
    fn path_query_defaults_to_query_string_behaviour() {
        for params in [
            vec![("endpoint", "https://example.test/search")],
            vec![
                ("endpoint", "https://example.test/search"),
                ("path_query", "false"),
            ],
        ] {
            let e = JsonApi::from_config("api", &config(&params), "test/1").unwrap();
            assert!(!e.path_query, "{params:?}");
            let url = resolve_endpoint(&e.endpoint, e.path_query, "a b", None).unwrap();
            assert_eq!(url.as_str(), "https://example.test/search");
            assert_eq!(
                e.request_params("a b", 5, None),
                vec![("q".to_string(), "a b".to_string())]
            );
        }
        // The option is also readable as a native TOML boolean.
        let config = crate::config::Config::from_toml_str(
            r#"
[engines.api]
type = "json_api"
endpoint = "https://example.test/words/{query}"
path_query = true
"#,
        )
        .unwrap();
        let e = JsonApi::from_config("api", &config.engines["api"], "test/1").unwrap();
        assert!(e.path_query);
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
            let error = JsonApi::from_config("api", &config(&params), "test/1")
                .err()
                .unwrap_or_else(|| panic!("{params:?} should be rejected"))
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[tokio::test]
    async fn path_query_sends_the_term_in_the_path() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/words/{{query}}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            let line = request.lines().next().unwrap_or_default();
            let body = r#"{"results":[]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            line.to_owned()
        });
        let e = JsonApi::from_config(
            "api",
            &config(&[("endpoint", &endpoint), ("path_query", "true")]),
            "test/1",
        )
        .unwrap();
        let results = e
            .search("rust async", 5, std::time::Duration::from_secs(1))
            .await
            .expect("provider responded");
        assert!(results.is_empty());
        // The term is in the path only: no `?q=` and no trailing `?`.
        assert_eq!(server.await.unwrap(), "GET /words/rust%20async HTTP/1.1");
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
    /// and an empty body, which surfaces as a parse error rather than silently
    /// passing, and `Connection: close` keeps every request on its own
    /// connection so the client cannot reorder them through its pool.
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
                        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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

    /// `count` distinct results, titled `T<first>..`, at `/<first>..`.
    fn json_page(count: usize, first: usize) -> String {
        let items = (first..first + count)
            .map(|index| format!(r#"{{"title":"T{index}","url":"https://example.test/{index}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"results":[{items}]}}"#)
    }

    /// Three usable results, but only after two entries that cannot be used.
    fn json_mixed_page(first: usize) -> String {
        let mut items = vec![
            r#"{"title":"  ","url":"https://example.test/blank"}"#.to_owned(),
            r#"{"title":"Unsafe","url":"javascript:alert(1)"}"#.to_owned(),
        ];
        items.extend((first..first + 3).map(|index| {
            format!(r#"{{"title":"T{index}","url":"https://example.test/{index}"}}"#)
        }));
        format!(r#"{{"results":[{}]}}"#, items.join(","))
    }

    /// `max_limit` items, in the shape `docker_hub` returns: a full page.
    fn docker_page(count: usize, first: usize) -> String {
        let items = (first..first + count)
            .map(|index| {
                format!(
                    r#"{{"name":"image{index}","slug":"image{index}","short_description":"d{index}"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"results":[{items}]}}"#)
    }

    #[tokio::test]
    async fn no_paging_keys_issue_exactly_one_request_with_an_identical_url() {
        // `max_pages` on its own, with no `page_param`, must stay inert.
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(5, 1)),
            Page::ok(json_page(5, 6)),
            Page::ok(json_page(5, 11)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("limit_param", "size"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e
            .search("rust async", 5, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(
            request_lines(task, &recorded).await,
            vec!["GET /search?q=rust+async&size=5 HTTP/1.1".to_owned()]
        );
        // The page-1 list, unchanged and in order.
        assert_eq!(
            results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["T1", "T2", "T3", "T4", "T5"]
        );
    }

    #[test]
    fn paging_defaults_are_off_and_single_page() {
        let e = JsonApi::from_config(
            "api",
            &config(&[("endpoint", "https://example.test/search")]),
            "test/1",
        )
        .unwrap();
        assert_eq!(e.paging.max_pages(), 1);
        assert_eq!(e.paging.page_for(1), None);
        assert_eq!(e.paging.page_for(7), None);
        assert_eq!(e.paging.first_page(), None);
        assert_eq!(e.paging.param(), None);
        // No page value is ever requested or sent.
        assert_eq!(
            resolve_endpoint("https://example.test/search", false, "q", None)
                .unwrap()
                .as_str(),
            "https://example.test/search"
        );
        assert_eq!(
            e.request_params("q", 5, e.paging.page_for(1)),
            vec![("q".to_string(), "q".to_string())]
        );
    }

    #[tokio::test]
    async fn first_page_satisfying_limit_issues_one_request() {
        for (limit, expected) in [(5, 5), (4, 4)] {
            let (addr, recorded, task) =
                recording_server(vec![Page::ok(json_page(5, 1)), Page::ok(json_page(5, 6))]).await;
            let endpoint = format!("{addr}/search");
            let e = JsonApi::from_config(
                "api",
                &config(&[
                    ("endpoint", &endpoint),
                    ("page_param", "first"),
                    ("max_pages", "3"),
                ]),
                "test/1",
            )
            .unwrap();
            let results = e
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
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(3, 1)),
            Page::ok(json_page(3, 4)),
            Page::ok(json_page(3, 7)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 5, Duration::from_secs(5)).await.unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[1], "GET /search?q=rust&page=2 HTTP/1.1");
        assert_eq!(results.len(), 5);
    }

    #[tokio::test]
    async fn a_page_that_parses_to_zero_results_stops_the_loop() {
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_mixed_page(1)),
            // An empty result array, and a page whose entries are all unusable:
            // either must end the loop instead of burning the page budget.
            Page::ok(r#"{"results":[]}"#),
            Page::ok(json_page(3, 7)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 100, Duration::from_secs(5)).await.unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 2);
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn a_later_page_is_parsed_at_the_page_size_not_the_remaining_need() {
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_mixed_page(1)),
            // The two unusable entries come first here: parsing this page at
            // the remaining need (2) would take them, drop them, and report a
            // healthy page as empty.
            Page::ok(json_mixed_page(4)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 5, Duration::from_secs(5)).await.unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 2);
        assert_eq!(
            results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["T1", "T2", "T3", "T4", "T5"]
        );
    }

    #[tokio::test]
    async fn max_pages_caps_the_number_of_requests() {
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(1, 1)),
            Page::ok(json_page(1, 2)),
            Page::ok(json_page(1, 3)),
            // A fourth page is queued and must never be requested.
            Page::ok(json_page(1, 4)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 100, Duration::from_secs(5)).await.unwrap();
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
            let error = JsonApi::from_config(
                "api",
                &config(&[
                    ("endpoint", "https://example.test/search"),
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
            let error = JsonApi::from_config(
                "api",
                &config(&[
                    ("endpoint", "https://example.test/search"),
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
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test/search"),
                ("page_param", "page"),
                ("max_pages", "10"),
            ]),
            "test/1",
        )
        .unwrap();
        assert_eq!(e.paging.max_pages(), 10);
        // ... and an unreadable value falls back to the documented default
        // instead of failing the engine, exactly like `max_limit`.
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test/search"),
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
            ("offset", "0", "40", ["offset=0", "offset=40", "offset=80"]),
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
            let (addr, recorded, task) = recording_server(vec![
                Page::ok(json_page(1, 1)),
                Page::ok(json_page(1, 2)),
                Page::ok(json_page(1, 3)),
            ])
            .await;
            let endpoint = format!("{addr}/search");
            let e = JsonApi::from_config(
                "api",
                &config(&[
                    ("endpoint", &endpoint),
                    ("page_param", param),
                    ("page_start", start),
                    ("page_step", step),
                    ("max_pages", "3"),
                ]),
                "test/1",
            )
            .unwrap();
            let results = e.search("rust", 100, Duration::from_secs(5)).await.unwrap();
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
        let (addr, recorded, task) = recording_server(vec![
            Page::slow(json_page(2, 1), Duration::from_millis(400)),
            Page::ok(json_page(2, 3)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "offset"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        // 600 ms of budget, 400 ms spent on page 1: 200 ms is left, below the
        // 500 ms floor, so page 2 is never started.
        let results = e
            .search("rust", 10, Duration::from_millis(600))
            .await
            .expect("page 1 results are still returned");
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn a_sub_floor_timeout_still_issues_exactly_one_request() {
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(1, 1)),
            Page::ok(json_page(1, 2)),
            Page::ok(json_page(1, 3)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e
            .search("rust", 100, Duration::from_millis(200))
            .await
            .unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn a_zero_timeout_still_issues_exactly_one_request() {
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(1, 1)),
            Page::ok(json_page(1, 2)),
            Page::ok(json_page(1, 3)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        // An exhausted budget must not fan out into a `max_pages` request
        // burst. At most one request is attempted, and no second page is ever
        // started; whether that one request even reaches the server before the
        // zero timeout aborts it is a race, so only the upper bound is asserted.
        let _ = e.search("rust", 100, Duration::ZERO).await;
        let lines = request_lines(task, &recorded).await;
        assert!(lines.len() <= 1, "{lines:?}");
    }

    #[tokio::test]
    async fn the_combined_list_is_truncated_to_limit() {
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(10, 1)),
            Page::ok(json_page(10, 11)),
            Page::ok(json_page(10, 21)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 25, Duration::from_secs(5)).await.unwrap();
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
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(3, 1)),
            Page::ok(json_page(4, 1)),
            Page::ok(json_page(1, 5)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 10, Duration::from_secs(5)).await.unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(
            results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["T1", "T2", "T3", "T4", "T5"]
        );

        // A provider that repeats its last page when exhausted (Bing) adds
        // nothing on page 2, which ends the loop.
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(3, 1)),
            Page::ok(json_page(3, 1)),
            Page::ok(json_page(1, 9)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 10, Duration::from_secs(5)).await.unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 2);
        assert_eq!(
            results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["T1", "T2", "T3"]
        );
    }

    #[tokio::test]
    async fn max_limit_still_clamps_every_page_request() {
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(3, 1)),
            Page::ok(json_page(3, 4)),
            Page::ok(json_page(3, 7)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("limit_param", "size"),
                ("max_limit", "10"),
                ("page_param", "offset"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 50, Duration::from_secs(5)).await.unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        for (line, offset) in lines.iter().zip(["1", "2", "3"]) {
            assert!(line.contains("&size=10&"), "{line}");
            assert!(
                line.ends_with(&format!("&offset={offset} HTTP/1.1")),
                "{line}"
            );
        }
        assert_eq!(results.len(), 9);
    }

    #[tokio::test]
    async fn a_full_page_at_the_max_limit_cap_does_not_end_the_loop() {
        // A real provider fills the page it is asked for, so page 1 returns
        // exactly `max_limit` items. `max_limit` caps the *page size*; the
        // running total is compared against the caller's `limit`, so the
        // guardrail must not trip here and paging must reach page 2 and 3.
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(10, 1)),
            Page::ok(json_page(10, 11)),
            Page::ok(json_page(10, 21)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("limit_param", "size"),
                ("max_limit", "10"),
                ("page_param", "from"),
                ("page_start", "0"),
                ("page_step", "10"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 25, Duration::from_secs(5)).await.unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        for (line, from) in lines.iter().zip(["0", "10", "20"]) {
            // The per-request page size is still clamped to `max_limit` ...
            assert!(line.contains("&size=10&"), "{line}");
            assert!(line.ends_with(&format!("&from={from} HTTP/1.1")), "{line}");
        }
        // ... while the total accumulates past it and is truncated to the
        // caller's `limit` instead of to the page cap.
        assert_eq!(results.len(), 25);
        assert!(results.len() > 10, "the total must exceed the page cap");
    }

    #[tokio::test]
    async fn a_max_limit_page_still_satisfies_a_smaller_limit_in_one_request() {
        // The other half of the guardrail: a page that fills `max_limit` also
        // satisfies a caller's `limit` below it, so exactly one request is made.
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(10, 1)),
            Page::ok(json_page(10, 11)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("limit_param", "size"),
                ("max_limit", "10"),
                ("page_param", "from"),
                ("page_start", "0"),
                ("page_step", "10"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 5, Duration::from_secs(5)).await.unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0], "GET /search?q=rust&size=5&from=0 HTTP/1.1");
        assert_eq!(results.len(), 5);
    }

    #[tokio::test]
    async fn the_shipped_docker_hub_entry_accumulates_past_its_page_cap() {
        // The exact trace from `docker_hub` (`limit_param = "size"`,
        // `max_limit = "10"`, upstream `?from=<n*10>`): a caller asking for 30
        // used to get exactly one request and 10 results.
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(docker_page(10, 1)),
            Page::ok(docker_page(10, 11)),
            Page::ok(docker_page(10, 21)),
        ])
        .await;
        let mut cfg = crate::config::Config::builtin_defaults().engines["docker_hub"].clone();
        cfg.params.insert(
            "endpoint".into(),
            toml::Value::from(format!("{addr}/api/search/v3/catalog/search")),
        );
        cfg.params
            .insert("page_param".into(), toml::Value::from("from"));
        cfg.params.insert("page_start".into(), toml::Value::from(0));
        cfg.params.insert("page_step".into(), toml::Value::from(10));
        cfg.params.insert("max_pages".into(), toml::Value::from(3));
        let e = JsonApi::from_config("docker_hub", &cfg, "test/1").unwrap();
        let results = e.search("rust", 30, Duration::from_secs(5)).await.unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 3, "{lines:?}");
        for (line, from) in lines.iter().zip(["0", "10", "20"]) {
            assert!(line.contains("&size=10&"), "{line}");
            assert!(line.ends_with(&format!("&from={from} HTTP/1.1")), "{line}");
        }
        assert_eq!(results.len(), 30);
        assert_eq!(results[29].title, "image30");
    }

    #[tokio::test]
    async fn a_max_limit_page_loop_still_obeys_the_shared_deadline() {
        let (addr, recorded, task) = recording_server(vec![
            Page::slow(json_page(2, 1), Duration::from_millis(400)),
            Page::ok(json_page(2, 3)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("limit_param", "size"),
                ("max_limit", "10"),
                ("page_param", "from"),
                ("page_start", "0"),
                ("page_step", "10"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        // Same budget arithmetic as the un-capped deadline test: 400 ms spent
        // on page 1 leaves 200 ms, below the 500 ms floor, so page 2 is never
        // started even though 8 more results are still wanted.
        let results = e
            .search("rust", 30, Duration::from_millis(600))
            .await
            .expect("page 1 results are still returned");
        let lines = request_lines(task, &recorded).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn paging_accepts_native_toml_booleans_and_integers() {
        let config = crate::config::Config::from_toml_str(
            r#"
[engines.api]
type = "json_api"
endpoint = "https://example.test/search"
page_param = "offset"
page_start = 0
page_step = 20
max_pages = 4
"#,
        )
        .unwrap();
        let e = JsonApi::from_config("api", &config.engines["api"], "test/1").unwrap();
        assert_eq!(e.paging.param(), Some("offset"));
        assert_eq!(e.paging.max_pages(), 4);
        assert_eq!(e.paging.page_for(1), Some(0));
        assert_eq!(e.paging.page_for(3), Some(40));
        assert_eq!(e.paging.first_page(), None);
        assert_eq!(
            e.request_params("q", 5, e.paging.page_for(2)),
            vec![
                ("q".to_string(), "q".to_string()),
                ("offset".to_string(), "20".to_string()),
            ]
        );

        let config = crate::config::Config::from_toml_str(
            r#"
[engines.api]
type = "json_api"
endpoint = "https://example.test/search/{page}"
page_in_path = true
page_start = 0
page_step = 10
max_pages = 4
"#,
        )
        .unwrap();
        let e = JsonApi::from_config("api", &config.engines["api"], "test/1").unwrap();
        assert_eq!(e.paging.first_page(), Some(0));
        assert_eq!(e.paging.param(), None);
    }

    #[tokio::test]
    async fn page_in_path_substitutes_decimal_digits_into_the_path() {
        for (template, start, step, expected) in [
            (
                "https://kickass.to/usearch/{query}/{page}/",
                "1",
                "35",
                [
                    "/usearch/a%2Fb/1/",
                    "/usearch/a%2Fb/36/",
                    "/usearch/a%2Fb/71/",
                ],
            ),
            (
                "https://x.test/search/{query}/page/{page}/",
                "1",
                "35",
                [
                    "/search/a%2Fb/page/1/",
                    "/search/a%2Fb/page/36/",
                    "/search/a%2Fb/page/71/",
                ],
            ),
            (
                "https://stocksnap.io/api/search-photos/{query}/relevance/desc/{page}",
                "1",
                "35",
                [
                    "/api/search-photos/a%2Fb/relevance/desc/1",
                    "/api/search-photos/a%2Fb/relevance/desc/36",
                    "/api/search-photos/a%2Fb/relevance/desc/71",
                ],
            ),
            // An offset in a path segment: page 1 is 0, never -10.
            (
                "https://x.test/{query}/{page}",
                "0",
                "10",
                ["/a%2Fb/0", "/a%2Fb/10", "/a%2Fb/20"],
            ),
        ] {
            // A local server is required for the absolute template, so the
            // host is replaced while the path template is kept as it is.
            let (addr, recorded, task) = recording_server(vec![
                Page::ok(json_page(1, 1)),
                Page::ok(json_page(1, 2)),
                Page::ok(json_page(1, 3)),
            ])
            .await;
            let suffix = template
                .split_once("://")
                .and_then(|(_, rest)| rest.find('/').map(|at| &rest[at..]))
                .unwrap_or("");
            let endpoint = format!("{addr}{suffix}");
            let e = JsonApi::from_config(
                "api",
                &config(&[
                    ("endpoint", &endpoint),
                    ("path_query", "true"),
                    ("page_in_path", "true"),
                    ("page_start", start),
                    ("page_step", step),
                    ("max_pages", "3"),
                ]),
                "test/1",
            )
            .unwrap_or_else(|error| panic!("{template}: {error}"));
            let results = e.search("a/b", 100, Duration::from_secs(5)).await.unwrap();
            let lines = request_lines(task, &recorded).await;
            assert_eq!(lines.len(), 3, "{template}: {lines:?}");
            for (line, want) in lines.iter().zip(expected) {
                assert!(line.starts_with(&format!("GET {want} HTTP/1.1")), "{line}");
            }
            assert_eq!(results.len(), 3, "{template}");
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
            ("https://x.test/{page}", "q", Some(1), "https://x.test/1"),
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
            let error = JsonApi::from_config("api", &config(&params), "test/1")
                .err()
                .unwrap_or_else(|| panic!("{params:?} should be rejected"))
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn a_zero_page_step_is_rejected_rather_than_looping_on_page_one() {
        // A zero step makes every page carry the identical value, so the loop
        // would spend a second request re-fetching page 1 and never advance.
        let error = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", "https://example.test/search"),
                ("page_param", "p"),
                ("page_step", "0"),
            ]),
            "test/1",
        )
        .err()
        .expect("page_step = 0 must be rejected")
        .to_string();
        assert!(error.contains("page_step must be at least 1"), "{error}");

        // `page_start = 0` stays legal: `docker_hub` sends `from=0` for page 1.
        assert!(
            JsonApi::from_config(
                "api",
                &config(&[
                    ("endpoint", "https://example.test/search"),
                    ("page_param", "from"),
                    ("page_start", "0"),
                    ("page_step", "10"),
                ]),
                "test/1",
            )
            .is_ok(),
            "page_start = 0 with a positive step must remain valid"
        );
    }

    #[test]
    fn an_unreadable_page_start_or_step_falls_back_to_the_documented_default() {
        // Same convention as `max_limit`: a value that is negative, non-numeric
        // or the wrong TOML type falls back to the documented default instead
        // of failing the whole engine.
        for (start, step, want_first, want_third) in [
            // An unreadable `page_start` falls back to 1; step 10 still applies.
            ("abc", "10", 1, 21),
            ("-5", "10", 1, 21),
            // An unreadable `page_step` falls back to 1, so pages step by one
            // from 0: page 3 is 0 + 2*1 = 2, NOT 0 + 2*10.
            ("0", "abc", 0, 2),
            ("0", "-5", 0, 2),
        ] {
            let e = JsonApi::from_config(
                "api",
                &config(&[
                    ("endpoint", "https://example.test/search"),
                    ("page_param", "p"),
                    ("page_start", start),
                    ("page_step", step),
                ]),
                "test/1",
            )
            .unwrap_or_else(|error| panic!("{start}/{step} should be accepted: {error}"));
            assert_eq!(
                e.paging.page_for(1),
                Some(want_first),
                "page 1 with page_start={start:?} page_step={step:?}"
            );
            assert_eq!(
                e.paging.page_for(3),
                Some(want_third),
                "page 3 with page_start={start:?} page_step={step:?}"
            );
        }
    }

    #[test]
    fn page_param_must_not_collide_with_a_query_or_limit_parameter() {
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
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("limit_param", "size"),
                    ("page_param", "size"),
                ],
                Some("collides with a configured query or limit parameter"),
            ),
            (
                vec![
                    ("endpoint", "https://example.test/search"),
                    ("page_param", "safe"),
                ],
                None,
            ),
        ] {
            match expected {
                Some(expected) => {
                    let error = JsonApi::from_config("api", &config(&params), "test/1")
                        .err()
                        .unwrap_or_else(|| panic!("{params:?} should be rejected"))
                        .to_string();
                    assert!(error.contains(expected), "{error}");
                }
                None => {
                    JsonApi::from_config("api", &config(&params), "test/1")
                        .expect("a non-colliding page parameter is accepted");
                }
            }
        }
    }

    #[test]
    fn page_param_must_not_collide_with_a_key_pinned_in_the_endpoint() {
        // Twelve shipped endpoints pin their page key literally (`gitea`'s
        // `?page=1`, `hex`'s `?page=1`, `marginalia`'s `?page=1`, `lemmy`'s
        // `?page=1`, ...), which is the natural way to enable paging on them.
        // reqwest's `query()` appends, so the computed value would ship as a
        // second `page` key, the provider would keep honouring the pinned
        // first occurrence and the loop would burn `max_pages` requests on
        // page 1. It is a config error, not a silent precedence.
        for (endpoint, page_param) in [
            (
                "https://gitea.com/api/v1/repos/search?sort=updated&order=desc&page=1",
                "page",
            ),
            (
                "https://hex.pm/api/packages/?sort=recent_downloads&page=1",
                "page",
            ),
            (
                "https://api2.marginalia-search.com/search?page=1&nsfw=1",
                "page",
            ),
            (
                "https://lemmy.ml/api/v3/search?page=1&type_=Communities",
                "page",
            ),
            ("https://example.test/search?offset=0", "offset"),
            (
                "https://learn.microsoft.com/api/search?search=rust&$skip=0",
                "$skip",
            ),
        ] {
            let error = JsonApi::from_config(
                "gitea",
                &config(&[
                    ("endpoint", endpoint),
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

        // A pinned key that is not the page parameter is not a duplicate.
        for (endpoint, page_param) in [
            ("https://example.test/search?nsfw=1", "page"),
            ("https://hex.pm/api/packages/?pages=1", "page"),
            // Same key, different case: the wire key is a different one.
            ("https://example.test/search?Page=1", "page"),
        ] {
            JsonApi::from_config(
                "api",
                &config(&[("endpoint", endpoint), ("page_param", page_param)]),
                "test/1",
            )
            .unwrap_or_else(|error| panic!("{endpoint} + page_param {page_param}: {error}"));
        }

        // `page_in_path` appends no query key, so a pinned `page` is fine there.
        JsonApi::from_config(
            "api",
            &config(&[
                (
                    "endpoint",
                    "https://example.test/search/{page}?nsfw=1&page=1",
                ),
                ("page_in_path", "true"),
            ]),
            "test/1",
        )
        .expect("a page in the path never duplicates an endpoint query key");

        // Paging off: the shipped endpoints keep loading unchanged, however
        // many keys they pin.
        let config = crate::config::Config::builtin_defaults();
        for name in ["gitea", "hex", "lemmy"] {
            JsonApi::from_config(name, &config.engines[name], "test/1")
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }

    #[tokio::test]
    async fn a_second_page_error_keeps_the_results_collected_so_far() {
        let (addr, recorded, task) =
            recording_server(vec![Page::ok(json_page(2, 1)), Page::status(500)]).await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 10, Duration::from_secs(5)).await.unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 2);
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn a_first_page_error_stays_strict_with_paging_on() {
        let (addr, recorded, task) = recording_server(vec![Page::status(500)]).await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "page"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        assert!(matches!(
            e.search("rust", 10, Duration::from_secs(5)).await,
            Err(EngineError::Http(_))
        ));
        assert_eq!(request_lines(task, &recorded).await.len(), 1);
    }

    #[tokio::test]
    async fn page_value_arithmetic_saturates_instead_of_overflowing() {
        let (addr, recorded, task) = recording_server(vec![
            Page::ok(json_page(1, 1)),
            Page::ok(json_page(1, 2)),
            Page::ok(json_page(1, 3)),
        ])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "offset"),
                ("page_start", "18446744073709551615"),
                ("page_step", "18446744073709551615"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 100, Duration::from_secs(5)).await.unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(results.len(), 3);
        for line in &lines {
            assert!(line.contains("offset=18446744073709551615"), "{line}");
        }
    }

    #[tokio::test]
    async fn duplicates_within_the_first_page_are_kept_when_paging_is_off() {
        // Deduplication is for *cross-page* repeats. With paging off the
        // output must stay exactly what the single request produced, including
        // a provider that lists the same URL twice on one page.
        let (addr, recorded, task) = recording_server(vec![Page::ok(
            r#"{"results":[
                {"title":"First","url":"https://example.test/1"},
                {"title":"Again","url":"https://example.test/1"},
                {"title":"Second","url":"https://example.test/2"}
            ]}"#,
        )])
        .await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[("endpoint", &endpoint), ("limit_param", "size")]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 10, Duration::from_secs(5)).await.unwrap();
        assert_eq!(request_lines(task, &recorded).await.len(), 1);
        assert_eq!(
            results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["First", "Again", "Second"]
        );
    }

    #[tokio::test]
    async fn a_zero_limit_still_issues_exactly_one_request() {
        let (addr, recorded, task) =
            recording_server(vec![Page::ok(json_page(3, 1)), Page::ok(json_page(3, 4))]).await;
        let endpoint = format!("{addr}/search");
        let e = JsonApi::from_config(
            "api",
            &config(&[
                ("endpoint", &endpoint),
                ("page_param", "offset"),
                ("page_start", "0"),
                ("page_step", "10"),
                ("max_pages", "3"),
            ]),
            "test/1",
        )
        .unwrap();
        let results = e.search("rust", 0, Duration::from_secs(5)).await.unwrap();
        let lines = request_lines(task, &recorded).await;
        assert_eq!(
            lines,
            vec!["GET /search?q=rust&offset=0 HTTP/1.1".to_owned()]
        );
        assert!(results.is_empty());
    }
}
