pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("sogou_wechat", "html_scrape", enabled = false, [
        "endpoint" => "https://weixin.sogou.com/weixin",
        "query_param" => "query",
        "param_page" => "1",
        "param_type" => "2",
        "result_selector" => "li[id*=\"sogou_vr_\"]",
        "link_selector" => "h3 a",
        "title_selector" => "h3 a",
        "snippet_selector" => "p.txt-info",
    ])
}
