pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("openverse", "json_api", [
        "endpoint" => "https://api.openverse.org/v1/images/?format=json",
        "query_param" => "q",
        "limit_param" => "page_size",
        "results_path" => "results",
        "title_field" => "title",
        "url_field" => "foreign_landing_url",
        "snippet_field" => "attribution",
    ])
}
