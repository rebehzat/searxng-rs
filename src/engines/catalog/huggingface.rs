pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("huggingface", "json_api", [
        "endpoint" => "https://huggingface.co/api/models?direction=-1",
        "query_param" => "search",
        "limit_param" => "limit",
        "title_field" => "id",
        "url_field" => "id",
        "url_prefix" => "https://huggingface.co",
        "snippet_field" => "description",
    ])
}
