// Bing Images via the server-rendered async list (searx
// `searx/engines/bing_images.py`, upstream commit
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// Endpoint provenance: upstream `request()` builds
// `https://www.bing.com/images/async?q=foo&mmasync=1&first=1&count=35` from
// `base_url` (`https://www.bing.com`); the `settings.yml` entry only carries
// the `bii` shortcut plus a commented-out `base_url: https://cn.bing.com`
// alternative for China-hosted instances, and sets no `disabled:`/`inactive:`
// flag, so the default host is the one used here. Upstream marks the engine
// `safesearch` but never sends a SafeSearch parameter in `request()`, so none
// is added here either.
//
// Structure, straight from upstream's `response()` xpath (translated 1:1 to
// CSS): items are `li` under `ul[contains(@class, "dgControl_list")]`, the
// title is `div.infnmpt//a` (text, falling back to `@title`), and the per
// item metadata JSON lives in the `m` attribute of `a.iusc`.
// The result URL upstream reports is `metadata["purl"]`. No adapter can read a
// URL out of a JSON blob embedded in an HTML attribute, so the link is taken
// from the publisher anchor in the very same list item,
// `div.imgpt div.lnkw a`, whose `href` a plain live `GET` of the endpoint
// above showed to be byte-identical to `purl` for all 35 items of a page
// (checked against the same markup upstream parses; no guessing involved).
// The snippet is that same anchor's text, i.e. upstream's `source` field
// (publisher host), used because upstream's `content` (`metadata["desc"]`)
// is likewise only available inside the `m` JSON.
//
// Deliberately not represented (neither adapter can express it, and guessing
// would be inaccurate): the `first=(pageno-1)*35+1` pagination, which is
// pinned here to upstream's first page; the per-request region parameters
// `setlang`/`cc` (upstream derives them from the caller's locale via
// `traits`, the adapter can only send static values); the per-request time
// range `qft=filterui:age-lt<minutes>`; the `enable_http3` hint; and the
// image-specific fields `img_src` (`murl`), `thumbnail_src` (`turl`),
// `resolution` and `img_format`, which live in the `m` JSON blob. Results
// are consequently web-shaped (title + publisher URL + publisher host) rather
// than image results. Title precedence is also inverted: upstream prefers the
// `div.infnmpt//a` text and only falls back to `@title`, while `title_attr`
// reads `@title` first and the text second; the two differ only where Bing
// truncates the visible text with an ellipsis.
//
// Enabled by default: upstream ships the engine active, the endpoint answers a
// plain unauthenticated `GET` with `200` and the full list, and this entry
// implements no CAPTCHA/anti-bot handling of any kind.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("bing_images", "html_scrape", [
        "endpoint" => "https://www.bing.com/images/async",
        "query_param" => "q",
        "param_mmasync" => "1",
        "param_first" => "1",
        "param_count" => "35",
        "result_selector" => "ul.dgControl_list > li",
        "link_selector" => ".imgpt .lnkw a",
        "title_selector" => ".infnmpt a",
        "title_attr" => "title",
        "snippet_selector" => ".imgpt .lnkw a",
    ])
}
