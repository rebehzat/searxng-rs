pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("deviantart", "html_scrape", enabled = false, [
        "endpoint" => "https://www.deviantart.com/search",
        "query_param" => "q",
        "result_selector" => "div[data-testid=\"content_row\"]",
        "link_selector" => "a[href][aria-label]",
        "title_attr" => "aria-label",
    ])
}
