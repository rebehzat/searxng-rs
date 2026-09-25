pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("astrophysics_data_system", "json_api", enabled = false, [
        // The adapter sends this environment variable verbatim, so it must contain
        // the complete ADS header value (for example, "Bearer <token>").
        "endpoint" => "https://api.adsabs.harvard.edu/v1/search/query?fl=abstract,bibcode,title&rows=10&sort=read_count%20desc",
        "query_param" => "q",
        "results_path" => "response.docs",
        "title_field" => "title",
        "normalize_title_html" => "true",
        "url_field" => "bibcode",
        "url_template" => "https://ui.adsabs.harvard.edu/abs/{value}/",
        "snippet_field" => "abstract",
        "normalize_snippet_html" => "true",
        "api_key_env" => "ASTROPHYSICS_DATA_SYSTEM_AUTHORIZATION",
        "api_key_header" => "Authorization",
    ])
}
