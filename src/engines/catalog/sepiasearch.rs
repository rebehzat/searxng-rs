pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("sepiasearch", "json_api", [
        "endpoint" => "https://sepiasearch.org/api/v1/search/videos?start=0&count=10&sort=-match&nsfw=false",
        "query_param" => "search",
        "results_path" => "data",
        "title_field" => "name",
        "url_field" => "url",
    ])
}
