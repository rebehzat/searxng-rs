pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("deviantart", "html_scrape", [
        "endpoint" => "https://www.deviantart.com/search",
        "query_param" => "q",
        "result_selector" => "div[data-testid=\"content_row\"] a[data-testid]",
        "link_selector" => "a[data-testid]",
        "title_selector" => "a[data-testid]",
    ])
}
