pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("sogou_videos", "json_api", enabled = false, [
        "endpoint" => "https://v.sogou.com/api/video/shortVideoV2?page=1&pagesize=10",
        "query_param" => "query",
        "max_limit" => "10",
        "results_path" => "data.list",
        "title_field" => "titleEsc",
        "url_field" => "url",
        "url_prefix" => "https://v.sogou.com",
        "snippet_field" => "site",
        "normalize_title_html" => "true",
    ])
}
