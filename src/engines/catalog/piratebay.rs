// The Pirate Bay torrents via the public apibay.org API (searx
// `searx/engines/piratebay.py`, upstream commit
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// First-page normalized subset: upstream issues a plain GET to
// `https://apibay.org/q.php?q={search_term}&cat={search_type}`, which answers
// with a top-level JSON array of torrent objects (no wrapper object, so no
// `results_path`). The title is upstream's `name`, and the link is rebuilt
// exactly as upstream does it, `url + "description.php?id=" + result["id"]`,
// with `url` being the settings.yml base `https://thepiratebay.org/`. No API
// key is required (`require_api_key: False` in upstream's `about` block).
//
// `cat` is pinned to `0` (files) in the endpoint. Upstream derives it from the
// requested category (`files`->`0`, `music`->`100`, `videos`->`200`), but the
// JSON adapter has no way to send per-request static query values; the engine
// is only registered for the `files` category upstream, so `0` is the value
// upstream would have used in practice.
//
// Deliberately not represented (the current adapters cannot express it, and
// guessing would be inaccurate): the `magnet:?xt=urn:btih:...` link built from
// `info_hash` plus the hardcoded tracker list (magnet URIs are not http(s) and
// the adapter rejects non-web result URLs), the seeder/leecher/size/published
// metadata and the `torrent.html` result template, the descending seeder sort
// (`sorted(..., key=itemgetter("seed"), reverse=True)`), and the "No results
// returned" sentinel check - apibay answers an empty search with a single
// placeholder object whose `name` is that string and whose `id` is still a
// usable integer, so such a response yields one link here instead of none.
// Fields the API omits (e.g. a missing `id`) are dropped by the adapter.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("piratebay", "json_api", [
        "endpoint" => "https://apibay.org/q.php?cat=0",
        "query_param" => "q",
        "title_field" => "name",
        "url_field" => "id",
        "url_template" => "https://thepiratebay.org/description.php?id={value}",
    ])
}
