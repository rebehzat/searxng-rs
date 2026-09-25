pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("radio_browser", "json_api", [
        "endpoint" => "https://de1.api.radio-browser.info/json/stations/search?order=votes&offset=0&hidebroken=true&reverse=true",
        "query_param" => "name",
        "limit_param" => "limit",
        "title_field" => "name",
        "url_field" => "homepage",
        "fallback_url_field" => "url_resolved",
        "fallback_url_template" => "{value}",
        "snippet_field" => "tags",
    ])
}
