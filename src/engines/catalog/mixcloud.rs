pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("mixcloud", "json_api", [
        "endpoint" => "https://api.mixcloud.com/search/?type=cloudcast&offset=0",
        "query_param" => "q",
        "limit_param" => "limit",
        "results_path" => "data",
        "title_field" => "name",
        "url_field" => "url",
        "snippet_field" => "user.name",
    ])
}
