pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("hex", "json_api", enabled = false, [
        "endpoint" => "https://hex.pm/api/packages/?sort=recent_downloads&page=1",
        "query_param" => "search",
        "limit_param" => "per_page",
        "max_limit" => "10",
        "title_field" => "name",
        "url_field" => "html_url",
        "snippet_field" => "meta.description",
    ])
}
