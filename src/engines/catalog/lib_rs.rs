pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("lib_rs", "html_scrape", enabled = false, [
        "endpoint" => "https://lib.rs/search",
        "query_param" => "q",
        "result_selector" => "main > div > ol > li",
        "link_selector" => "a",
        "title_selector" => "a > div.h > h4",
        "snippet_selector" => "a > div.h > p",
    ])
}
