// Google News, the mobile (`/wml/`) variant of Google Search with `tbm=nws`.
// Transcribed from `searx/engines/google_news.py` in upstream searxng
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3, which delegates request building
// to `google_request()` in `searx/engines/google.py`:
//
//   params["url"] = f"https://www.google.com/wml/search?{urlencode(args)}"
//   args = {"q": query, "sca_esv": "1", **locale_params, "tbm": "nws"}
//
// Response is parsed by `response()` as an (X)HTML document: the result unit
// is the *anchor itself*, `//a[contains(@href, "/url?q=")]`, with the title in
// a `span.M3vVJe` (older markup: `span.fuLhoc`), the publisher in
// `span.dXDvrc`, the publication date in `span.YVIcad`, and the thumbnail in
// `img[src*="encrypted-tbn"]`. `unwrap_google_url()` rewrites the relative
// `/url?q=<target>&sa=U...` href into the real publisher URL.
// `settings.yml` carries no `base_url`/query-param override for this engine
// (it is enabled by default and only sets `shortcut: gon`), so the
// `google.py` request builder above is the authority for the endpoint and the
// `q` parameter.
//
// DISABLED: the engine is not reachable by an honest plain HTTP request.
// Upstream only gets real results by sending `impersonate: "chrome99_android"`
// (browser/TLS fingerprint spoofing) together with a randomly chosen
// Nokia/Symbian User-Agent to hit the legacy `/wml/` frontend - both are
// identity/fingerprint spoofing and are out of bounds here, and upstream's own
// `wml_dom()` calls `detect_google_sorry()` to catch the bot-protection
// "sorry" pages this invites. A plain GET with an honest, unmodified
// User-Agent to the exact URL above returns a JavaScript-only interstitial
// (`/httpservice/retry/enablejs`, zero `/url?q=` result markup), so the
// html_scrape adapter would only ever report a soft parse error. Enabling this
// entry therefore needs an access route that does not involve impersonation.
//
// Not represented (the adapters cannot express it, and guessing would be
// inaccurate):
//   - URL unwrapping: html_scrape resolves the relative `/url?q=...` href
//     against the endpoint, so result URLs stay Google redirect wrappers
//     instead of the publisher URL. Only `bing`'s `/ck/a` unwrap exists.
//   - The upstream dedup by unwrapped URL, the `google.com/search` URL filter,
//     and the "skip entries whose title span is missing" rule.
//   - Paging: upstream sends `start=(pageno-1)*10` (max_page 50); the adapter
//     has no pagination. This entry is first-page only.
//   - The `lr`/`cr` locale parameters (`use_locales=False` is passed for news,
//     so upstream drops them too) and time-range/safesearch, which upstream
//     explicitly disables for this engine.
//   - The `img[src*="encrypted-tbn"]` thumbnail: per-result metadata has no
//     field in the html_scrape adapter.
//   - The `<?xml ...?>` prologue stripping in `wml_dom()`; the selector set
//     below is a CSS transcription of the upstream XPath, using a comma group
//     to express upstream's `A or B` span fallbacks (the adapter takes the
//     first match in document order, so snippet yields source *or* date
//     rather than upstream's "source / date" join).
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("google_news", "html_scrape", enabled = false, [
        "endpoint" => "https://www.google.com/wml/search",
        "query_param" => "q",
        "param_sca_esv" => "1",
        "param_tbm" => "nws",
        "result_selector" => "a[href*=\"/url?q=\"]",
        "link_selector" => ":scope",
        "title_selector" => "span.M3vVJe, span.fuLhoc",
        "snippet_selector" => "span.dXDvrc, span.YVIcad",
    ])
}
