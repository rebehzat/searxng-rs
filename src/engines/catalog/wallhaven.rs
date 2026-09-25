// Wallhaven wallpapers via the official public JSON API
// (searx `searx/engines/wallhaven.py`, upstream commit
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// First-page normalized subset: the search listing is a JSON object whose
// results live under `data`, each wallpaper becomes one result, and the result
// link is the wallpaper page from upstream's `result['url']`
// (`https://wallhaven.cc/w/<id>`). The listing is fixed at 24 entries per page
// and the API accepts no page-size parameter, so no limit parameter is sent
// here either; the adapter truncates to the caller's limit client-side.
// `page=1` is pinned in the endpoint because the JSON adapter has no paging
// parameter, which is the honest representation of upstream's first page.
//
// Purity is pinned to `100` (SFW only) in the endpoint. Upstream derives
// `purity` from the caller's safesearch level (`0/1/2` -> `111`/`110`/`100`),
// which the JSON adapter cannot express per request, and only this engine's own
// upstream `safesearch_map` keys the setting to one of those values. The
// documented API default is already SFW-only, so pinning the most restrictive
// value keeps every result safe-for-work for any caller and never widens
// results beyond what the API offers without a key. For the same reason no
// API key is wired up: NSFW purities require a key, and the API applies the
// account holder's browsing settings to keyed searches, which could widen
// results past the pinned `purity=100`.
//
// Deliberately not represented (the adapter has no support for it, and guessing
// would be inaccurate): upstream's `images.html` result template and its
// `img_src` (`path`), `thumbnail_src` (`thumbs.small`), `resolution`,
// `publishedDate` (`created_at`), `img_format` (`file_type`) and `filesize`
// fields. Wallhaven returns no title at all (upstream sends an empty string),
// so the always-present full image URL (`path`) is used as the title and the
// wallpaper `category` as the snippet, rather than the upstream
// `"<category> / <purity>"` content string, which the adapter cannot compose
// from two fields. With `purity` pinned to SFW the purity half of that string
// would be the constant `sfw` anyway. The upstream `wh` shortcut is also not
// carried over: engine shortcuts are not part of this catalog's vocabulary.
//
// Disabled by default: upstream ships the engine inactive and it is an
// NSFW-capable image provider, so it stays opt-in.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("wallhaven", "json_api", enabled = false, [
        "endpoint" => "https://wallhaven.cc/api/v1/search?purity=100&page=1",
        "query_param" => "q",
        "results_path" => "data",
        "title_field" => "path",
        "url_field" => "url",
        "snippet_field" => "category",
    ])
}
