pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("swisscows_news", "json_api", enabled = false, [
        // The adapter exposes the first Swisscows News page with the upstream
        // defaults. Locale, freshness, and later pages are intentionally not
        // configurable in this catalog subset.
        "endpoint" => "https://api.swisscows.com/news/search?itemsCount=20&region=de-DE&language=de&offset=0&freshness=All&sortOrder=Desc&sortBy=Created",
        "query_param" => "query",
        "results_path" => "items",
        "title_field" => "title",
        "normalize_title_html" => "true",
        "url_field" => "uri",
        "snippet_field" => "description",
    ])
}
