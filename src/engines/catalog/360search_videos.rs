pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("360search_videos", "json_api", enabled = false, [
        "endpoint" => "https://tv.360kan.com/v1/video/list?count=10&start=0",
        "query_param" => "q",
        "results_path" => "data.result",
        "title_field" => "title",
        "url_field" => "play_url",
        "snippet_field" => "description",
        "normalize_title_html" => "true",
        "normalize_snippet_html" => "true",
    ])
}
