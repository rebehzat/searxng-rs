// Bing News via the server-rendered infinite-scroll endpoint used by upstream
// searx `searx/engines/bing_news.py` (commit 3cd69d30). Upstream's `about` block
// claims `"results": "RSS"`, but the module's `request()`/`response()` pair
// actually GETs `https://www.bing.com/news/infinitescrollajax` and parses the
// returned HTML fragment, so this is an `html_scrape` definition.
//
// Endpoint/params transcribed from `request()` for the first page only
// (`pageno = 1`): `q=<query>`, `InfiniteScroll=1`, `first=1`, `SFX=0`,
// `form=PTFTNR`. The selectors are transcribed from `response()`:
// `//div[contains(@class,"newsitem")]` -> `div[class*="newsitem"]`,
// `.//a[@class="title"]` -> `a.title` (upstream takes the title from the
// anchor's own text, which the adapter does by default), and
// `.//div[@class="snippet"]` -> `div.snippet`.
//
// Omitted because the adapters cannot express it:
// - Paging. Upstream walks `first`/`SFX` (page * n + 1 / page) because Bing
//   repeats the last page when exhausted; html_scrape has no pagination, so
//   this is a faithful first-page subset only.
// - `mkt`, which upstream derives per request from `searxng_locale` via
//   `get_locale_params()`. There is no dynamic parameter support, so no market
//   is forced and Bing falls back to its own locale default.
// - `qft` time-range filtering (`time_range_support` / `time_map`).
// - Per-result `metadata` (source `aria-label` + link `data-author`, joined
//   with " | "), the `imagelink` thumbnail with its `https://www.bing.com`
//   prefixing, and the EngineTraits region patch for `zh-CN`.
//
// No bot detection, token, or cookie handling is involved: this is a single
// plain HTTP GET, exactly as upstream performs it. Upstream settings.yml sets
// no `disabled:`/`inactive:` flag for bing news, so it ships enabled.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("bing_news", "html_scrape", [
        "endpoint" => "https://www.bing.com/news/infinitescrollajax",
        "query_param" => "q",
        "param_InfiniteScroll" => "1",
        "param_first" => "1",
        "param_SFX" => "0",
        "param_form" => "PTFTNR",
        "result_selector" => "div[class*=\"newsitem\"]",
        "link_selector" => "a.title",
        "snippet_selector" => "div.snippet",
    ])
}
