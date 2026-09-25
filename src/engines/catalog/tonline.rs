pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("tonline", "html_scrape", enabled = false, [
        "endpoint" => "https://suche.t-online.de/web",
        "query_param" => "q",
        "param_mandant" => "toi",
        "param_dia" => "suche",
        "param_ptl" => "std",
        "param_page" => "1",
        "result_selector" => "div#google_re > div.doc",
        "link_selector" => ":scope > a[href]",
        "title_selector" => ".tMMReshl",
        "snippet_selector" => ".tMMRest",
    ])
}
