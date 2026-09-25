pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("searchrockit", "html_scrape", enabled = false, [
        "endpoint" => "https://searchrockit.com/results/web",
        "query_param" => "q",
        "param_p" => "1",
        "result_selector" => ".results-list .result-item",
        "link_selector" => ".result-item--title",
        "title_selector" => ".result-item--title",
        "snippet_selector" => ".result-item--desc",
    ])
}
