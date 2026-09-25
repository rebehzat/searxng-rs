pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("neocities", "html_scrape", enabled = false, [
        "endpoint" => "https://neocities.org/search",
        "query_param" => "q",
        "result_selector" => "div.result-item",
        "link_selector" => ".result-url a",
        "title_selector" => "h3.result-title a",
        "snippet_selector" => "p.result-snippet",
    ])
}
