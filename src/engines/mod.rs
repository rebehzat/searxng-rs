//! Engine abstraction and the built-in registry.

pub mod duckduckgo;
pub mod html_scrape;
pub mod json_api;
pub mod wikipedia;

use std::time::Duration;

use async_trait::async_trait;

use crate::config::EngineConfig;
use crate::error::EngineResult;
use crate::models::SearchResult;

/// A search backend. Implementations translate a query into normalized
/// results and must never attempt to defeat bot protection: no CAPTCHA
/// solving, no fingerprint spoofing, no IP rotation.
#[async_trait]
pub trait Engine: Send + Sync {
    /// Name reported on results and in status output.
    fn name(&self) -> &str;

    /// Run one query. `limit` is a best-effort hint; engines may return
    /// fewer results. `timeout` bounds the whole request + parse.
    async fn search(
        &self,
        query: &str,
        limit: usize,
        timeout: Duration,
    ) -> EngineResult<Vec<SearchResult>>;
}

/// Build a concrete engine from its configuration entry.
///
/// Returns `Err` for unknown `type` values so callers can report the
/// misconfiguration instead of silently dropping the engine.
pub fn build_engine(
    name: &str,
    cfg: &EngineConfig,
    user_agent: &str,
) -> anyhow::Result<Box<dyn Engine>> {
    match cfg.engine_type.as_str() {
        "duckduckgo_html" | "ddg_html" => Ok(Box::new(duckduckgo::DuckDuckGoHtml::new(
            name,
            user_agent,
            &cfg.string_param("region", "wt-wt"),
        ))),
        "wikipedia" | "wikipedia_rest" => Ok(Box::new(wikipedia::WikipediaRest::new(
            name,
            user_agent,
            &cfg.string_param("language", "en"),
        ))),
        "html_scrape" => Ok(Box::new(html_scrape::HtmlScrape::from_config(
            name, cfg, user_agent,
        )?)),
        "json_api" => Ok(Box::new(json_api::JsonApi::from_config(
            name, cfg, user_agent,
        )?)),
        other => Err(anyhow::anyhow!(
            "engine '{name}' has unknown type '{other}' (supported: duckduckgo_html, wikipedia, html_scrape, json_api)"
        )),
    }
}
