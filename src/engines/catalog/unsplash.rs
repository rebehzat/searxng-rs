pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("unsplash", "json_api", enabled = false, [
        "endpoint" => "https://unsplash.com/napi/search/photos?page=1&per_page=20",
        "query_param" => "query",
        "results_path" => "results",
        "title_field" => "alt_description",
        "url_field" => "links.html",
        "snippet_field" => "description",
    ])
}
