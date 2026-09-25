pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("peertube", "json_api", enabled = false, [
        "endpoint" => "https://peer.tube/api/v1/search/videos?searchTarget=search-index&resultType=videos&start=0&count=10&sort=-match&nsfw=both",
        "query_param" => "search",
        "results_path" => "data",
        "title_field" => "name",
        "url_field" => "url",
        "snippet_field" => "description",
    ])
}
