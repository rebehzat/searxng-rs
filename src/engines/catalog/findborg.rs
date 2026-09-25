pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("findborg", "json_api", enabled = false, [
        "endpoint" => "https://www.findborg.com/apis/proxy.php?type=search",
        "query_param" => "q",
        "results_path" => "data.web.results",
        "title_field" => "title",
        "url_field" => "url",
        "snippet_field" => "description",
        "normalize_snippet_html" => "true",
    ])
}
