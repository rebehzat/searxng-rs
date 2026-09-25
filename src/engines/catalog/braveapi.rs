pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("braveapi", "json_api", enabled = false, [
        "endpoint" => "https://api.search.brave.com/res/v1/web/search?offset=0&text_decorations=false&safesearch=strict",
        "query_param" => "q",
        "limit_param" => "count",
        "max_limit" => "20",
        "results_path" => "web.results",
        "title_field" => "title",
        "url_field" => "url",
        "snippet_field" => "description",
        "api_key_env" => "BRAVE_API_KEY",
        "api_key_header" => "X-Subscription-Token",
    ])
}
