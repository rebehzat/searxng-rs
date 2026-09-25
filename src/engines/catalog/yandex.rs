pub fn definition() -> crate::engines::catalog::CatalogEntry {
    // First-page web results; locale, time-range, paging, and published-date
    // handling are intentionally outside this adapter's normalized subset.
    crate::engine_catalog_entry!("yandex", "html_scrape", enabled = false, [
        "endpoint" => "https://yandex.com/search/site/",
        "query_param" => "text",
        "param_tmpl_version" => "releases",
        "param_web" => "1",
        "param_frame" => "1",
        "param_searchid" => "3131712",
        "result_selector" => "li.b-serp-item",
        "link_selector" => "a.b-serp-item__title-link",
        "title_selector" => "h3.b-serp-item__title",
        "snippet_selector" => ".b-serp-item__content .b-serp-item__text",
    ])
}
