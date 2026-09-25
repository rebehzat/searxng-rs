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
    snippet_field: String,
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
            snippet_field: cfg.string_param("snippet_field", "snippet"),
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
                let title = text_at(item, &self.title_field)?;
                let mut url = text_at(item, &self.url_field)?;
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
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                let url = normalized_web_url(&url)?;
                let snippet = text_at(item, &self.snippet_field).filter(|s| !s.is_empty());
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
        let mut params = vec![(self.query_param.clone(), query.to_owned())];
        if let Some(limit_param) = &self.limit_param {
            params.push((limit_param.clone(), limit.to_string()));
        }
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

fn text_at(value: &Value, path: &str) -> Option<String> {
    let value = value_at(value, path)?;
    value.as_str().map(str::to_owned).or_else(|| {
        value
            .as_array()?
            .iter()
            .find_map(Value::as_str)
            .map(str::to_owned)
    })
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
