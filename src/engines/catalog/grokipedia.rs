pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("grokipedia", "json_api", enabled = false, [
        "endpoint" => "https://grokipedia.com/api/full-text-search?offset=0",
        "query_param" => "query",
        "limit_param" => "limit",
        "max_limit" => "10",
        "results_path" => "results",
        "title_field" => "title",
        "url_field" => "slug",
        "url_template" => "https://grokipedia.com/page/{value}",
        "snippet_field" => "snippet",
        "normalize_snippet_html" => "true",
    ])
}
