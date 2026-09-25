pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("ebay", "html_scrape", enabled = false, [
        "endpoint" => "https://www.ebay.com/sch/i.html",
        "query_param" => "_nkw",
        "param__sacat" => "1",
        "result_selector" => "li[class~=\"s-item\"]",
        "link_selector" => "a.s-item__link",
        "title_selector" => "h3.s-item__title",
        "snippet_selector" => "div[span=\"SECONDARY_INFO\"]",
    ])
}
