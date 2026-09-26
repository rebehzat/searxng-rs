//! Engine abstraction and the built-in registry.

pub mod catalog;
pub mod duckduckgo;
pub mod html_scrape;
pub mod json_api;
pub mod wikipedia;

use std::borrow::Cow;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;

use crate::config::EngineConfig;
use crate::error::EngineResult;
use crate::models::SearchResult;

/// Literal placeholder replaced by the percent-encoded search term when the
/// `path_query` option is enabled.
pub(crate) const QUERY_PLACEHOLDER: &str = "{query}";

/// Stand-in term used to validate a `path_query` endpoint template at
/// configuration time, before any real query exists.
pub(crate) const ENDPOINT_PROBE: &str = "probe";

/// Resolve an endpoint template for `query`.
///
/// When `path_query` is set, every `{query}` occurrence is replaced with the
/// percent-encoded search term *in the raw string*, because a literal `{` is
/// not a valid URL character and `Url::parse` would silently rewrite it to
/// `%7B`. `urlencoding::encode` escapes every byte outside the RFC 3986
/// unreserved set (`A-Za-z0-9-._~`, space as `%20`), so a search term can
/// never add a path segment, a query string, or a fragment. This is stricter
/// than upstream Python `quote()`, which keeps `/` literal.
///
/// Pure by design: it takes no `self`, so a later page-aware change only has
/// to add a `page` argument and a `{page}` placeholder here. Shared by the
/// [`json_api`] and [`html_scrape`] adapters so the two can never diverge.
pub(crate) fn resolve_endpoint(
    template: &str,
    path_query: bool,
    query: &str,
) -> anyhow::Result<Url> {
    let resolved = if path_query {
        let encoded: Cow<'_, str> = urlencoding::encode(query);
        template.replace(QUERY_PLACEHOLDER, encoded.as_ref())
    } else {
        template.to_owned()
    };
    Url::parse(&resolved).map_err(Into::into)
}

/// Validate the `path_query` / `{query}` placeholder combination.
///
/// Returns `Err` for the three misconfigurations that would otherwise produce
/// a silently broken engine: the flag on with no placeholder, the placeholder
/// present with the flag off, and a template that is not a valid URL once
/// substituted. With the flag off and no placeholder this is exactly the
/// pre-existing endpoint parse, so behaviour is unchanged for every config
/// that does not opt in.
pub(crate) fn validate_endpoint(
    name: &str,
    endpoint: &str,
    path_query: bool,
) -> anyhow::Result<Url> {
    let has_placeholder = endpoint.contains(QUERY_PLACEHOLDER);
    if path_query && !has_placeholder {
        anyhow::bail!(
            "engine '{name}': path_query is enabled but the endpoint has no {QUERY_PLACEHOLDER} placeholder"
        );
    }
    if has_placeholder && !path_query {
        anyhow::bail!(
            "engine '{name}': the endpoint has a {QUERY_PLACEHOLDER} placeholder but path_query is not enabled"
        );
    }
    // A `path_query` endpoint is only ever used after substitution, so
    // validate the substituted form; the raw template is not a URL.
    resolve_endpoint(endpoint, path_query, ENDPOINT_PROBE)
        .map_err(|error| anyhow::anyhow!("engine '{name}': invalid endpoint: {error}"))
}

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
        )?)),
        "wikipedia" | "wikipedia_rest" => Ok(Box::new(wikipedia::WikipediaRest::new(
            name,
            user_agent,
            &cfg.string_param("language", "en"),
        )?)),
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
