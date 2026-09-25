pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("pypi", "html_scrape", [
        "endpoint" => "https://pypi.org/search/",
        "query_param" => "q",
        "param_page" => "1",
        "result_selector" => "main ul > li",
        "link_selector" => "a.package-snippet",
        "title_selector" => "h3 .package-snippet__name",
        "snippet_selector" => "p",
    ])
}
