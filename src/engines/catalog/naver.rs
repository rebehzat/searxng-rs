pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("naver", "html_scrape", enabled = false, [
        "endpoint" => "https://search.naver.com/search.naver",
        "query_param" => "query",
        "param_where" => "web",
        "param_start" => "1",
        "result_selector" => "div.fds-web-normal-doc-root",
        "link_selector" => "a[href^=\"http\"]:not([href*=\"keep.naver.com\"])",
        "title_selector" => ".sds-comps-text-type-headline1",
        "snippet_selector" => ".sds-comps-text-type-body1",
    ])
}
