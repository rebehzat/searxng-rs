// Piped video search (searx `searx/engines/piped.py`, upstream commit
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// First-page normalized subset: query the configured Piped backend's
// `/search` endpoint with the pinned `videos` filter, then map `items` to the
// adapter's title, URL, and short-description fields. The relative result URL
// is resolved against the configured frontend as upstream does.
//
// The current JSON adapter cannot choose randomly among multiple upstream
// backend instances, so this entry deliberately uses the second (currently
// reachable) backend pinned in settings.yml. It also cannot represent upstream's
// next-page token, thumbnails, upload dates, view counts, durations, embeds, or
// video template.
// Those behaviors are omitted rather than approximated; this is the initial
// `/search` response only.
//
// Disabled by default because the engine is marked `inactive: true` upstream.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("piped", "json_api", enabled = false, [
        "endpoint" => "https://api.piped.private.coffee/search?filter=videos",
        "query_param" => "q",
        "results_path" => "items",
        "title_field" => "title",
        "url_field" => "url",
        "url_prefix" => "https://srv.piped.video",
        "snippet_field" => "shortDescription",
    ])
}
