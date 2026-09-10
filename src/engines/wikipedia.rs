//! Wikipedia MediaWiki API adapter.
//!
//! This uses the documented public API and does not scrape or circumvent
//! access controls.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::engines::Engine;
use crate::error::EngineResult;
use crate::models::SearchResult;

pub struct WikipediaRest {
    name: String,
    client: reqwest::Client,
    language: String,
}

impl WikipediaRest {
    pub fn new(name: &str, user_agent: &str, language: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(user_agent.to_string())
            .build()
            .expect("static client configuration");
        Self {
            name: name.to_string(),
            client,
            language: if language.trim().is_empty() {
                "en".into()
            } else {
                language.into()
            },
        }
    }

    pub fn endpoint(&self) -> String {
        format!("https://{}.wikipedia.org/w/api.php", self.language)
    }

    fn parse_response(&self, body: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        let response: ApiResponse = serde_json::from_str(body)?;
        Ok(response
            .query
            .search
            .into_iter()
            .take(limit)
            .map(|item| {
                let mut metadata = HashMap::new();
                metadata.insert("language".into(), self.language.clone());
                SearchResult::with_metadata(
                    self.name.clone(),
                    item.title.clone(),
                    format!(
                        "https://{}.wikipedia.org/wiki/{}",
                        self.language,
                        item.title.replace(' ', "_")
                    ),
                    Some(strip_html(&item.snippet)),
                    metadata,
                )
            })
            .collect())
    }
}

#[async_trait]
impl Engine for WikipediaRest {
    fn name(&self) -> &str {
        &self.name
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
        timeout: Duration,
    ) -> EngineResult<Vec<SearchResult>> {
        let limit_string = limit.min(50).to_string();
        let response = self
            .client
            .get(self.endpoint())
            .query(&[
                ("action", "query"),
                ("list", "search"),
                ("srsearch", query),
                ("format", "json"),
                ("utf8", "1"),
                ("srlimit", limit_string.as_str()),
            ])
            .timeout(timeout)
            .send()
            .await?
            .error_for_status()?;
        let body = response.text().await?;
        self.parse_response(&body, limit)
            .map_err(|_| crate::error::EngineError::Parse)
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    query: Query,
}
#[derive(Debug, Deserialize)]
struct Query {
    search: Vec<SearchItem>,
}
#[derive(Debug, Deserialize)]
struct SearchItem {
    title: String,
    snippet: String,
}

fn strip_html(input: &str) -> String {
    let fragment = scraper::Html::parse_fragment(input);
    fragment
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_payload() {
        let e = WikipediaRest::new("wikipedia", "test/1", "en");
        let results = e.parse_response(r#"{"query":{"search":[{"title":"Rust","snippet":"A <span>systems</span> language"}]}}"#, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].snippet.as_deref(), Some("A systems language"));
        assert_eq!(results[0].metadata["language"], "en");
    }

    #[test]
    fn endpoint_uses_language() {
        let e = WikipediaRest::new("wiki", "test/1", "de");
        assert_eq!(e.endpoint(), "https://de.wikipedia.org/w/api.php");
    }
}
