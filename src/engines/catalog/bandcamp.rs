pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("bandcamp", "html_scrape", enabled = false, [
        "endpoint" => "https://bandcamp.com/",
        "query_param" => "q",
        "param_page" => "1",
        "result_selector" => "li.searchresult",
        "link_selector" => ".itemurl a",
        "title_selector" => ".heading a",
        "snippet_selector" => ".subhead",
    ])
}
