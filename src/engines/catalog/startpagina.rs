pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("startpagina", "json_api", enabled = false, [
        "endpoint" => "https://search.kompas.services/api/v2/search/web/?page_size=10&page=1",
        "query_param" => "q",
        "results_path" => "results",
        "title_field" => "title",
        "url_field" => "original_url",
        "snippet_field" => "description",
    ])
}
