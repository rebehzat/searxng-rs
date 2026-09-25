pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("pexels", "json_api", enabled = false, [
        "endpoint" => "https://www.pexels.com/en-us/api/v3/search/photos",
        "query_param" => "query",
        "limit_param" => "per_page",
        "max_limit" => "80",
        "results_path" => "data",
        "title_field" => "attributes.title",
        "url_field" => "attributes.url",
        "snippet_field" => "attributes.description",
    ])
}
