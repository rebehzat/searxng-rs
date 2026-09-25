//! Configurable JSON adapter for documented provider APIs.
//!
//! Providers may expose results under a dotted path such as `message.items`;
//! result fields may also be dotted (`primary_location.landing_page_url`).
//! This adapter performs ordinary public or API-key-authenticated requests;
//! it never circumvents provider controls.

use std::collections::HashMap;
use std::time::Duration;

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
        let api_key = match cfg.params.get("api_key_env") {
            Some(toml::Value::String(env_name)) if !env_name.is_empty() => std::env::var(env_name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (cfg.string_param("api_key_header", "Authorization"), value)),
            _ => None,
        };
        let limit_param = cfg.string_param("limit_param", "").trim().to_owned();
        Ok(Self {
            name: name.into(),
            client: reqwest::Client::builder().user_agent(user_agent).build()?,
            endpoint,
            query_param: cfg.string_param("query_param", "q"),
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
            max_limit: cfg.string_param("max_limit", "").parse::<usize>().ok(),
            snippet_field: cfg.string_param("snippet_field", "snippet"),
            array_value_field: (!cfg.string_param("array_value_field", "").is_empty())
                .then(|| cfg.string_param("array_value_field", "")),
            normalize_title_html: cfg.string_param("normalize_title_html", "") == "true",
            normalize_snippet_html: cfg.string_param("normalize_snippet_html", "") == "true",
            snippet_max_length: cfg
                .string_param("snippet_max_length", "")
                .parse::<usize>()
                .ok(),
            api_key,
        })
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
                if title.is_empty() {
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
                    .filter(|snippet| !snippet.is_empty())
                    .map(|snippet| {
                        let snippet = if self.normalize_snippet_html {
                            html_to_text(&snippet)
                        } else {
                            snippet
                        };
                        truncate_text(snippet, self.snippet_max_length)
                    })
                    .filter(|snippet| !snippet.is_empty());
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
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
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
        let template = self.fallback_url_template.as_deref()?;
        let fallback = template.replace("{value}", value).replace("{url}", value);
        self.build_result_url(item, &fallback, false)
    }

    fn request_params(&self, query: &str, limit: usize) -> Vec<(String, String)> {
        let limit = self.max_limit.map_or(limit, |max| limit.min(max));
        let mut params = vec![(self.query_param.clone(), query.to_owned())];
        if let Some(limit_param) = &self.limit_param {
            params.push((limit_param.clone(), limit.to_string()));
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
        let params = self.request_params(query, limit);
        let limit = self.max_limit.map_or(limit, |max| limit.min(max));
        let mut request = self
            .client
            .get(&self.endpoint)
            .query(&params)
            .timeout(timeout);
        if let Some((header, value)) = &self.api_key {
            request = request.header(header, value);
        }
        let body = request.send().await?.error_for_status()?.text().await?;
        let value: Value = serde_json::from_str(&body).map_err(|_| EngineError::Parse)?;
        Ok(self.parse_value(value, limit))
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
            e.request_params("rust async", 7),
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
            e.request_params("rust", 50),
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
}
