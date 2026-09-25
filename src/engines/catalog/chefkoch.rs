// Chefkoch recipes via the provider's own public JSON search gateway
// (searx `searx/engines/chefkoch.py`, upstream commit
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// Endpoint and parameters are transcribed from upstream: `base_url =
// "https://api.chefkoch.de"` plus the `request()` path
// `/v2/search-gateway/recipes?query=<q>&limit=20&offset=<n>`, so the query
// parameter really is `query` (not `q`) and the page size really is 20
// (`page_size`), pinned here as `max_limit`. The payload is a top-level object
// whose `results` array holds one `recipe` object per hit, hence the dotted
// `recipe.*` field paths and the `results_path`.
//
// First-page normalized subset: json_api has no pagination, so the `offset`
// parameter and upstream's `paging = True` are omitted and this is a
// first-page-only view.
//
// Deliberately not represented (the adapters cannot express it, and guessing
// would be inaccurate):
//  * the `skip_premium` filter on `recipe.isPremium` / `recipe.isPlus` -- there
//    is no per-result predicate, so premium/plus recipes are included here;
//  * upstream's composed `content` string, which concatenates `recipe.subtitle`
//    with difficulty, preparation time and ingredient count -- only
//    `recipe.subtitle` survives as the snippet, and only when present;
//  * the per-result `thumbnail` (with `<format>` replaced by the
//    `crop-240x300` template) and the `publishedDate` parsed from
//    `recipe.submissionDate` -- the adapter attaches no per-result metadata.
//
// Disabled by default: verified 2026-09-25 from this deployment that the
// endpoint answers `HTTP 403` with an empty body to an honest plain request
// (`curl -A 'curl/8.5.0'`), and equally to a browser User-Agent, so this is an
// edge refusal rather than a client-identity check. No result page can be read
// without working around the provider's access controls, which is out of scope.
// The transcription below is therefore UNVERIFIED against a live response
// (upstream requires no API key, token, cookie or challenge flow, and upstream
// ships the engine enabled) and must be re-checked before being enabled.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("chefkoch", "json_api", enabled = false, [
        "endpoint" => "https://api.chefkoch.de/v2/search-gateway/recipes",
        "query_param" => "query",
        "limit_param" => "limit",
        "max_limit" => "20",
        "results_path" => "results",
        "title_field" => "recipe.title",
        "url_field" => "recipe.siteUrl",
        "snippet_field" => "recipe.subtitle",
    ])
}
