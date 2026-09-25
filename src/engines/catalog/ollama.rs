pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("ollama", "html_scrape", enabled = false, [
        "endpoint" => "https://ollama.com/search",
        "query_param" => "q",
        "result_selector" => "ul.grid > li",
        "link_selector" => "a[href^=\"/library/\"]",
        "title_selector" => "h2",
        "snippet_selector" => "p.max-w-lg.break-words.text-neutral-800.text-md",
    ])
}
