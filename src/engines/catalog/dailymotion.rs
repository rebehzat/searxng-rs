pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("dailymotion", "json_api", [
        "endpoint" => "https://api.dailymotion.com/videos?fields=title,url,description,thumbnail_360_url,id,created_time,duration",
        "query_param" => "search",
        "results_path" => "list",
        "title_field" => "title",
        "url_field" => "url",
        "snippet_field" => "description",
    ])
}
