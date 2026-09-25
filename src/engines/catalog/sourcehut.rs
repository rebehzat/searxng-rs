pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("sourcehut", "html_scrape", enabled = false, [
        "endpoint" => "https://sr.ht/projects",
        "query_param" => "search",
        "param_page" => "1",
        "param_sort" => "recently-updated",
        "result_selector" => "div.event-list > div.event",
        "link_selector" => "h4 > a:nth-of-type(2)",
        "title_selector" => "h4",
        "snippet_selector" => "p",
    ])
}
