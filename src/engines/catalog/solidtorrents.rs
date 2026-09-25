// SolidTorrents torrent search (searx `searx/engines/solidtorrents.py`,
// upstream commit 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// Endpoint provenance: upstream `request()` builds `base_url + '/search' +
// '?q=<query>&page=<pageno>'` with the query parameter `q`; upstream
// settings.yml overrides `base_url` with the mirror list
// [https://solidtorrents.to, https://bitsearch.to] and sets `timeout: 4.0`.
// The catalog can hold only one endpoint, so the first listed mirror is used
// verbatim; solidtorrents.to currently 301-redirects to bitsearch.to. The
// random mirror pick, the settings-level 4s timeout and paging beyond the
// first page are not expressible (`html_scrape` takes one static endpoint and
// one static `param_*` value), so the entry is pinned to page 1 and to the
// first mirror.
//
// Selectors transcribed verbatim from upstream `response()` XPath:
//   result     //li[contains(@class, "search-result")]
//   title      //h5[contains(@class, "title")]
//   url        //h5[contains(@class, "title")]/a/@href   (relative; upstream
//              prefixes base_url, the adapter resolves against the endpoint)
//   stats      //div[contains(@class, "stats")]/div
//
// Deliberately not represented (the adapter has no per-result metadata, no
// second link and no result filter): upstream's `dl-torrent` / `dl-magnet`
// hrefs (torrent file + magnet URI - the adapter emits a single URL per
// result, the detail page, so magnet links are unreachable from the agent
// output), the `//a[contains(@class, "category")]` metadata, the
// filesize/seed/leech/publishedDate stats split (stats[1], stats[2], stats[3],
// stats[4]; the `.stats` block is emitted as one whitespace-collapsed snippet
// instead), the `torrent.html` result template, and upstream's rule that drops
// entries lacking a torrent-file/magnet pair (non-torrent "anime" rows cannot
// be filtered out here, so they may appear with the detail-page URL).
//
// Disabled by default: an honest plain HTTP GET to the upstream mirror from
// this environment is refused at the edge - solidtorrents.to redirects to
// bitsearch.to, which answers HTTP 429 with Cloudflare "error code: 1015"
// (IP rate limit) on every attempt, so no result page can be read. Working
// around that would mean defeating the provider's access controls, which is
// out of scope; the definition ships disabled until the engine is reachable by
// a plain request.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("solidtorrents", "html_scrape", enabled = false, [
        "endpoint" => "https://solidtorrents.to/search",
        "query_param" => "q",
        "param_page" => "1",
        "result_selector" => "li.search-result",
        "link_selector" => "h5.title a",
        "title_selector" => "h5.title",
        "snippet_selector" => ".stats",
    ])
}
