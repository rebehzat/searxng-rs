pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("mwmbl", "json_api", [
        "endpoint" => "https://api.mwmbl.org/api/v1/search/",
        "query_param" => "s",
        "title_field" => "title",
        "url_field" => "url",
        "snippet_field" => "extract",
    ])
}
