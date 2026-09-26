//! Engine abstraction and the built-in registry.

pub mod catalog;
pub mod duckduckgo;
pub mod html_scrape;
pub mod json_api;
pub mod wikipedia;

use std::borrow::Cow;
use std::time::{Duration, Instant};

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

/// Literal placeholder replaced by the percent-encoded page number when
/// `page_in_path` is enabled. Mirrors [`QUERY_PLACEHOLDER`].
pub(crate) const PAGE_PLACEHOLDER: &str = "{page}";

/// Hard ceiling on `max_pages`.
///
/// A configuration outside `1..=MAX_PAGES_CEILING` is **rejected**, never
/// silently clamped: a clamped cap would hide a typo behind a request count
/// nobody can predict, while the rejection names the key, the range and the
/// value and is reported per engine instead of aborting the run.
pub(crate) const MAX_PAGES_CEILING: usize = 10;

/// Smallest budget handed to a single page request: the floor of the
/// remaining-deadline slice, and the test for "the caller's budget is spent".
pub(crate) const MIN_REQUEST_BUDGET: Duration = Duration::from_millis(500);

/// Multi-page configuration shared verbatim by [`json_api`] and
/// [`html_scrape`].
///
/// [`Paging::none`] is what every existing configuration gets, and it keeps
/// both adapters on their pre-change single-request path: `max_pages()` is 1
/// and `page_for()` returns `None`, so no page value is ever computed, sent or
/// substituted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Paging {
    /// Query-string parameter carrying the page number (`None` when the page
    /// travels in the path, or when paging is off).
    param: Option<String>,
    /// True when the page travels in the URL path via [`PAGE_PLACEHOLDER`].
    in_path: bool,
    /// Value sent for page 1.
    start: usize,
    /// Increment added per page.
    step: usize,
    /// Requests allowed for one search.
    max_pages: usize,
}

impl Paging {
    /// Paging disabled: exactly one request, no page value anywhere.
    pub(crate) const fn none() -> Self {
        Self {
            param: None,
            in_path: false,
            start: 1,
            step: 1,
            max_pages: 1,
        }
    }

    /// Query-string parameter carrying the page number, if any.
    pub(crate) fn param(&self) -> Option<&str> {
        self.param.as_deref()
    }

    /// Wire value for the 1-based `page`, or `None` when paging is disabled.
    ///
    /// `None` is also what leaves [`PAGE_PLACEHOLDER`] untouched in
    /// [`resolve_endpoint`].
    pub(crate) fn page_for(&self, page: usize) -> Option<usize> {
        if self.param.is_none() && !self.in_path {
            return None;
        }
        // Saturating: `page_start = 0` with `page_step > 0` is legal
        // (`docker_hub` sends `from=0` for page 1), so `page - 1` must never
        // wrap, and a mistyped huge `page_step` must never panic.
        Some(
            self.start
                .saturating_add(page.saturating_sub(1).saturating_mul(self.step)),
        )
    }

    /// Value substituted for [`PAGE_PLACEHOLDER`] when the endpoint template is
    /// validated at configuration time, or `None` when the page does not travel
    /// in the path.
    pub(crate) fn first_page(&self) -> Option<usize> {
        self.in_path.then_some(self.start)
    }

    /// Hard cap on requests for one search; `1` means "paging off".
    pub(crate) const fn max_pages(&self) -> usize {
        self.max_pages
    }
}

/// Read and validate the optional paging keys.
///
/// `page_param` arrives already trimmed and `None` when the key is absent or
/// blank, so an empty `page_param = ""` is indistinguishable from "not
/// configured" - the same convention used for `limit_param` and `path_query`.
///
/// `reserved` lists parameter names that must not be re-used as the page
/// parameter, because emitting the same key twice would send the provider the
/// first (stale) value and the loop would never leave page 1.
///
/// `page_start`, `page_step` and `max_pages` follow `max_limit` and fall back
/// to their documented default when the value is unreadable (negative,
/// non-numeric, wrong TOML type) rather than failing the whole engine.
pub(crate) fn paging_from_config(
    name: &str,
    cfg: &EngineConfig,
    page_param: Option<String>,
    page_in_path: bool,
    reserved: &[&str],
) -> anyhow::Result<Paging> {
    if page_param.is_some() && page_in_path {
        anyhow::bail!("engine '{name}': page_param and page_in_path are mutually exclusive");
    }
    // Validated even when paging is off, so the key is never silently inert.
    let max_pages = cfg.usize_param("max_pages", Some(3)).unwrap_or(3);
    if !(1..=MAX_PAGES_CEILING).contains(&max_pages) {
        anyhow::bail!(
            "engine '{name}': max_pages must be between 1 and {MAX_PAGES_CEILING}, got {max_pages}"
        );
    }
    if let Some(param) = &page_param
        && reserved.contains(&param.as_str())
    {
        anyhow::bail!(
            "engine '{name}': page_param '{param}' collides with a configured query or limit parameter"
        );
    }
    // A zero step makes every page carry the identical value, so the loop can
    // only ever re-request page 1: it spends a second request and can never
    // advance. `page_start = 0` is legal (docker_hub sends `from=0`), so only
    // the step is rejected.
    let step = cfg.usize_param("page_step", Some(1)).unwrap_or(1);
    if step == 0 {
        anyhow::bail!("engine '{name}': page_step must be at least 1, got 0");
    }
    // Default off: the adapters keep their exact pre-change code path.
    if page_param.is_none() && !page_in_path {
        return Ok(Paging::none());
    }
    Ok(Paging {
        param: page_param,
        in_path: page_in_path,
        start: cfg.usize_param("page_start", Some(1)).unwrap_or(1),
        step,
        max_pages,
    })
}

/// Reject a `page_param` that the endpoint already pins in its own query
/// string.
///
/// Both adapters **append** the computed page value to the endpoint's query
/// string (`Url::query_pairs_mut().append_pair` in [`html_scrape`], reqwest's
/// `RequestBuilder::query` in [`json_api`]; neither replaces an existing key).
/// A key that is already pinned in the endpoint therefore ships twice -
/// `?sort=updated&page=1&q=..&limit=10&page=2` - and providers honour the first
/// occurrence, so the loop would keep re-requesting page 1 and silently burn
/// the whole `max_pages` budget on it. That is the same failure mode as the
/// `param_*` duplicate that [`html_scrape`] removes, and the same shape as the
/// `query_param`/`limit_param` collision [`paging_from_config`] rejects: a
/// configuration error with no silent precedence, so the catalog author
/// deletes the pinned key from the endpoint.
///
/// `endpoint` is the already-validated URL (with any `{query}`/`{page}`
/// substitution applied), so its query string is the one the request will
/// carry. A `page_in_path` engine has no `page_param` and is unaffected, and
/// with paging off `Paging::none()` has no `param` either, so this can only
/// ever fire for a configuration that opted into paging.
pub(crate) fn reject_pinned_page_param(
    name: &str,
    endpoint: &Url,
    paging: &Paging,
) -> anyhow::Result<()> {
    let Some(page_param) = paging.param() else {
        return Ok(());
    };
    if endpoint.query_pairs().any(|(key, _)| key == page_param) {
        anyhow::bail!(
            "engine '{name}': page_param '{page_param}' is already present in the endpoint's query string ({endpoint}); remove the pinned key from the endpoint"
        );
    }
    Ok(())
}

/// Resolve an endpoint template for `query` and an optional page value.
///
/// When `path_query` is set, every `{query}` occurrence is replaced with the
/// percent-encoded search term *in the raw string*, because a literal `{` is
/// not a valid URL character and `Url::parse` would silently rewrite it to
/// `%7B`. `urlencoding::encode` escapes every byte outside the RFC 3986
/// unreserved set (`A-Za-z0-9-._~`, space as `%20`), so a search term can
/// never add a path segment, a query string, or a fragment. This is stricter
/// than upstream Python `quote()`, which keeps `/` literal.
///
/// `page` replaces every [`PAGE_PLACEHOLDER`] occurrence the same way. The
/// value is computed as a `usize` and can only be ASCII digits, for which
/// `urlencoding::encode` is provably a no-op; running it anyway means even a
/// future string-sourced page value could not add a path segment, a query
/// string or a fragment. `page: None` leaves the placeholder untouched, which
/// is why a `{page}` endpoint must be rejected at configuration time.
///
/// Pure by design: it takes no `self`. Shared by the [`json_api`] and
/// [`html_scrape`] adapters so the two can never diverge.
pub(crate) fn resolve_endpoint(
    template: &str,
    path_query: bool,
    query: &str,
    page: Option<usize>,
) -> anyhow::Result<Url> {
    let mut resolved: Cow<'_, str> = if path_query {
        let encoded: Cow<'_, str> = urlencoding::encode(query);
        Cow::Owned(template.replace(QUERY_PLACEHOLDER, encoded.as_ref()))
    } else {
        Cow::Borrowed(template)
    };
    if let Some(page) = page {
        let value = page.to_string();
        let encoded: Cow<'_, str> = urlencoding::encode(&value);
        resolved = Cow::Owned(resolved.replace(PAGE_PLACEHOLDER, encoded.as_ref()));
    }
    Url::parse(&resolved).map_err(Into::into)
}

/// Validate the `path_query` / `{query}` and `page_in_path` / `{page}`
/// placeholder combinations.
///
/// Returns `Err` for the misconfigurations that would otherwise produce a
/// silently broken engine: a flag on with no placeholder, a placeholder present
/// with the flag off, and a template that is not a valid URL once substituted.
/// `page` is the real first-page value for the `page_in_path` case, so a
/// `page_start = 0` template is genuinely probed.
///
/// With both flags off and neither placeholder present this is exactly the
/// pre-existing endpoint parse, so behaviour is unchanged for every config that
/// does not opt in.
pub(crate) fn validate_endpoint(
    name: &str,
    endpoint: &str,
    path_query: bool,
    page: Option<usize>,
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
    let has_page_placeholder = endpoint.contains(PAGE_PLACEHOLDER);
    if page.is_some() && !has_page_placeholder {
        anyhow::bail!(
            "engine '{name}': page_in_path is enabled but the endpoint has no {PAGE_PLACEHOLDER} placeholder"
        );
    }
    if has_page_placeholder && page.is_none() {
        anyhow::bail!(
            "engine '{name}': the endpoint has a {PAGE_PLACEHOLDER} placeholder but page_in_path is not enabled"
        );
    }
    // A `path_query` / `page_in_path` endpoint is only ever used after
    // substitution, so validate the substituted form; the raw template is not
    // a URL.
    resolve_endpoint(endpoint, path_query, ENDPOINT_PROBE, page)
        .map_err(|error| anyhow::anyhow!("engine '{name}': invalid endpoint: {error}"))
}

/// Remaining budget for the next page request, or `None` once the caller's
/// whole-search budget is too small to start another page.
///
/// [`Engine::search`] documents that `timeout` bounds the whole request +
/// parse, so a multi-page loop must share that one budget instead of
/// multiplying it. The caller never gets a fresh `timeout` per page, and
/// running out of budget returns what was collected rather than a failure.
///
/// `first` exempts the opening request from the floor, for two reasons: a
/// caller with a budget below [`MIN_REQUEST_BUDGET`] must still get the single
/// request it always got, and reaching this point has already consumed a sliver
/// of the budget, so testing `remaining >= MIN_REQUEST_BUDGET` strictly would
/// drop that first request. Pages after it need at least `MIN_REQUEST_BUDGET`
/// left, and never a zero budget - otherwise an exhausted (or zero) budget
/// would fan out into a full `max_pages` request burst. A caller whose whole
/// budget is smaller than the floor therefore gets exactly one request, which
/// is the correct outcome rather than a dead branch.
pub(crate) fn request_budget(deadline: Instant, first: bool) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    // A page after the first must have at least `MIN_REQUEST_BUDGET` left, so
    // an exhausted budget can never fan out into a request burst. Callers that
    // ask for less than `MIN_REQUEST_BUDGET` up front get exactly one request:
    // `remaining` is then always below the floor, which is the correct outcome
    // rather than a dead branch.
    let affordable = first || (remaining > Duration::ZERO && remaining >= MIN_REQUEST_BUDGET);
    affordable.then_some(remaining)
}

/// Normalized key for cross-page deduplication.
///
/// Both adapters already store normalized absolute URLs (`normalized_web_url`
/// in `json_api`, `base.join` in `html_scrape`), so the stored string *is* the
/// normalization. Shared so the two adapters can never dedupe differently.
pub(crate) fn dedupe_url_key(url: &str) -> &str {
    url
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
