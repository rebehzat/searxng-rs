// Mastodon account search on the default instance (searx
// `searx/engines/mastodon.py`, upstream commit
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// Provenance: upstream `request()` builds
//   {base_url}/api/v2/search?q=<query>&resolve=false&type=accounts&limit=40
// with `base_url = "https://mastodon.social"` (also the real host in
// settings.yml, which sets `mastodon_type: accounts` and no timeout override);
// `response()` iterates the `accounts` array of the returned `Search` object.
//
// The json_api adapter can only emit the query parameter and one limit
// parameter, so the static `resolve=false` and `type=accounts` values cannot
// be sent. Neither changes the result set obtained here: `resolve=false` is the
// documented no-resolution behaviour, and `results_path = "accounts"` selects
// exactly the accounts array that `type=accounts` would have produced.
// `page_size = 40` is carried over as `limit_param`/`max_limit`; Mastodon caps
// unauthenticated account search results, and neither adapter nor upstream can
// paginate (the Search API requires OAuth for `offset`, which we do not do).
//
// Deliberately not represented (adapters cannot express it, and guessing would
// be inaccurate): the composite upstream title `"{username} ({followers_count}
// followers)"` - the adapter reads a single field, so the title is the bare
// `username`; the `avatar` thumbnail and the `created_at` publication date,
// since json_api emits no per-result metadata; and the `hashtags` variant of
// the upstream engine (upstream `mastodon_type`), which this single default
// `accounts` entry does not cover. `note` is raw HTML upstream; it is
// normalized to visible text here.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("mastodon", "json_api", [
        "endpoint" => "https://mastodon.social/api/v2/search",
        "query_param" => "q",
        "limit_param" => "limit",
        "max_limit" => "40",
        "results_path" => "accounts",
        "title_field" => "username",
        "url_field" => "uri",
        "snippet_field" => "note",
        "normalize_snippet_html" => "true",
    ])
}
