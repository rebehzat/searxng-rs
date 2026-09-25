pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("archlinux", "html_scrape", [
        "endpoint" => "https://wiki.archlinux.de/index.php?",
        "query_param" => "search",
        "param_title" => "Spezial:Suche",
        "param_limit" => "20",
        "param_profile" => "default",
        "result_selector" => "ul.mw-search-results > li",
        "link_selector" => ".mw-search-result-heading a",
        "title_selector" => ".mw-search-result-heading a",
        "snippet_selector" => ".searchresult",
    ])
}
