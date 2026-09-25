pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("gitea", "json_api", enabled = false, [
        "endpoint" => "https://gitea.com/api/v1/repos/search?sort=updated&order=desc&page=1",
        "query_param" => "q",
        "limit_param" => "limit",
        "max_limit" => "10",
        "results_path" => "data",
        "title_field" => "full_name",
        "url_field" => "html_url",
        "snippet_field" => "description",
    ])
}
