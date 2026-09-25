pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("shopify_stock", "html_scrape", [
        "endpoint" => "https://www.shopify.com/stock-photos/photos/search",
        "query_param" => "q",
        "param_page" => "1",
        "result_selector" => "div[class*=\"js-masonry-grid\"] > div",
        "link_selector" => "a[class*=\"photo-tile\"]",
        "title_selector" => "p[class*=\"photo-tile__title\"]",
    ])
}
