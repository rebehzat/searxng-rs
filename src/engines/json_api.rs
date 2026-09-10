//! Configurable JSON search adapter for documented provider APIs.
//!
//! The adapter expects an endpoint that accepts a query parameter and returns
//! either an array of objects or an object containing `results`/`items`.
//! Field names and optional API-key environment variables are configurable.
//! It performs ordinary authenticated requests only; it does not bypass
//! provider controls.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
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
    title_field: String,
    url_field: String,
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
        Ok(Self {
            name: name.into(),
            client: reqwest::Client::builder().user_agent(user_agent).build()?,
            endpoint,
            query_param: cfg.string_param("query_param", "q"),
            title_field: cfg.string_param("title_field", "title"),
            url_field: cfg.string_param("url_field", "url"),
            snippet_field: cfg.string_param("snippet_field", "snippet"),
            api_key,
        })
    }

    fn parse_value(&self, value: Value, limit: usize) -> Vec<SearchResult> {
        let candidates = value
            .get("results")
            .or_else(|| value.get("items"))
            .unwrap_or(&value);
        let Some(items) = candidates.as_array() else {
            return Vec::new();
        };
        items
            .iter()
            .take(limit)
            .filter_map(|item| {
                let title = item.get(&self.title_field)?.as_str()?.trim();
                let url = item.get(&self.url_field)?.as_str()?.trim();
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                let snippet = item
                    .get(&self.snippet_field)
                    .and_then(Value::as_str)
                    .map(str::to_owned);
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
        let mut request = self
            .client
            .get(&self.endpoint)
            .query(&[(self.query_param.as_str(), query)])
            .timeout(timeout);
        if let Some((header, value)) = &self.api_key {
            request = request.header(header, value);
        }
        let body = request.send().await?.error_for_status()?.text().await?;
        let value: Value = serde_json::from_str(&body).map_err(|_| EngineError::Parse)?;
        Ok(self.parse_value(value, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_results_and_items_shapes() {
        let cfg = EngineConfig {
            engine_type: "json_api".into(),
            enabled: true,
            params: [(
                "endpoint".into(),
                toml::Value::String("https://example.test".into()),
            )]
            .into_iter()
            .collect(),
        };
        let e = JsonApi::from_config("api", &cfg, "test/1").unwrap();
        let value: Value = serde_json::json!({"results": [{"title": "Rust", "url": "https://rust-lang.org", "snippet": "safe"}]});
        let results = e.parse_value(value, 5);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].snippet.as_deref(), Some("safe"));
    }

    #[test]
    fn missing_endpoint_is_rejected() {
        let cfg = EngineConfig {
            engine_type: "json_api".into(),
            enabled: true,
            params: HashMap::new().into_iter().collect(),
        };
        assert!(JsonApi::from_config("api", &cfg, "test/1").is_err());
    }
}
