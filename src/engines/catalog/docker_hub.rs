// Docker Hub catalog search (searx `searx/engines/docker_hub.py`, upstream
// commit 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// Endpoint provenance: upstream's `request()` builds
// `https://hub.docker.com/api/search/v3/catalog/search` with
// `query=<terms>`, `from=<page_size * (pageno - 1)>` and `size=<page_size>`
// (page_size = 10). The upstream `settings.yml` block only pins the name,
// shortcut and categories, so the `base_url` and query parameter name in the
// Python module are the ground truth. This is a public, unauthenticated JSON
// GET endpoint (`require_api_key: False`, `use_official_api: True`), so no
// credentials, tokens or bot-evasion are involved.
//
// First-page normalized subset: results live under the top-level `results`
// array; `name` is the title, `short_description` the snippet, and the result
// URL is `https://hub.docker.com` + a prefix + `slug`.
//
// Deliberately not represented (the adapters cannot express it):
//  - Paging. Upstream sends a `from` offset per page; the json_api adapter has
//    no offset/pagination parameter, so only the first page is fetched. The
//    upstream page size of 10 is kept via `max_limit`.
//  - The conditional URL prefix. Upstream emits `/_/<slug>` when
//    `source` is `store` or `official`, and `/r/<slug>` otherwise. The adapter
//    supports one static `url_prefix` (or a single `url_template` with a
//    source field interpolated verbatim), not a membership test on
//    `source`, so the `/r/` form is used for every hit. Official
//    `library/*` images then reach the same page through Docker Hub's own
//    `301` to `/_/<slug>`.
//  - Per-result metadata the adapter has no field for: `thumbnail`
//    (`logo_url.large`/`logo_url.small`), `package_name`, `maintainer`
//    (`publisher.name`), `publishedDate` (`updated_at`/`created_at`),
//    `popularity` (`star_count` plus each `rate_plans[].repositories[].pull_count`)
//    and `tags` (the flattened `rate_plans[].architectures[].name` list), and
//    the `packages.html` result template.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("docker_hub", "json_api", [
        "endpoint" => "https://hub.docker.com/api/search/v3/catalog/search",
        "query_param" => "query",
        "limit_param" => "size",
        "max_limit" => "10",
        "results_path" => "results",
        "title_field" => "name",
        "url_field" => "slug",
        "url_prefix" => "https://hub.docker.com/r/",
        "snippet_field" => "short_description",
    ])
}
