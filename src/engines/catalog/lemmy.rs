pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("lemmy", "json_api", [
        "endpoint" => "https://lemmy.ml/api/v3/search?page=1&type_=Communities",
        "query_param" => "q",
        "results_path" => "communities",
        "title_field" => "community.title",
        "url_field" => "community.actor_id",
    ])
}
