pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("jina", "json_api", enabled = false, [
        // The adapter sends the environment variable verbatim, so this must
        // contain the complete header value (for example, "Bearer <token>").
        "endpoint" => "https://s.jina.ai/?page=1&engine=reader&respondWith=no-content",
        "query_param" => "q",
        "results_path" => "data",
        "title_field" => "title",
        "url_field" => "url",
        "snippet_field" => "description",
        "api_key_env" => "JINA_AUTHORIZATION",
        "api_key_header" => "Authorization",
    ])
}
