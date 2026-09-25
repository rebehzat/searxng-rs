pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("marginalia", "json_api", enabled = false, [
        // Pin the provider's experimental extreme-result reduction because this
        // adapter has no per-request safesearch setting.
        "endpoint" => "https://api2.marginalia-search.com/search?page=1&nsfw=1",
        "query_param" => "query",
        "limit_param" => "count",
        "max_limit" => "20",
        "results_path" => "results",
        "title_field" => "title",
        "url_field" => "url",
        "snippet_field" => "description",
        "api_key_env" => "MARGINALIA_API_KEY",
        "api_key_header" => "API-Key",
    ])
}
