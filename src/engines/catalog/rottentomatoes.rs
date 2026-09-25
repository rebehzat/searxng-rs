pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("rottentomatoes", "html_scrape", enabled = false, [
        "endpoint" => "https://www.rottentomatoes.com/search",
        "query_param" => "search",
        "result_selector" => "search-page-media-row",
        "link_selector" => "a[href]",
        "title_selector" => "a img[alt]",
        "title_attr" => "alt",
    ])
}
