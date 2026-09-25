pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("google_scholar", "html_scrape", [
        "endpoint" => "https://scholar.google.com/scholar",
        "query_param" => "q",
        "param_start" => "0",
        "param_as_sdt" => "2007",
        "param_as_vis" => "0",
        "result_selector" => "div[data-rp]",
        "link_selector" => "h3 a",
        "title_selector" => "h3 a",
        "snippet_selector" => ".gs_rs",
    ])
}
