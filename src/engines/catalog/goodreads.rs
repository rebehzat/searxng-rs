pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("goodreads", "html_scrape", enabled = false, [
        "endpoint" => "https://www.goodreads.com/search",
        "query_param" => "q",
        "param_page" => "1",
        "result_selector" => "table tr",
        "link_selector" => "a.bookTitle",
        "title_selector" => "a.bookTitle",
        "snippet_selector" => "span.uitext",
    ])
}
