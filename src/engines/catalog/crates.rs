pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("crates", "json_api", enabled = false, [
        "endpoint" => "https://crates.io/api/v1/crates",
        "query_param" => "q",
        "limit_param" => "per_page",
        "results_path" => "crates",
        "title_field" => "name",
        "url_field" => "name",
        "url_prefix" => "https://crates.io/crates/",
        "snippet_field" => "description",
    ])
}
