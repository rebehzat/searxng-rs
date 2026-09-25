pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("btdigg", "html_scrape", enabled = false, [
        "endpoint" => "https://btdig.com/search",
        "query_param" => "q",
        "param_p" => "0",
        "result_selector" => "div.one_result",
        "link_selector" => "div.torrent_name a",
        "snippet_selector" => "div.torrent_excerpt",
    ])
}
