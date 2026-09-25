pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("ccc_media", "json_api", enabled = false, [
        "endpoint" => "https://api.media.ccc.de/public/events/search?page=1",
        "query_param" => "q",
        "results_path" => "events",
        "title_field" => "title",
        "url_field" => "frontend_link",
        "snippet_field" => "description",
    ])
}
