pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("bpb", "json_api", enabled = false, [
        "endpoint" => "https://www.bpb.de/bpbapi/filter/search?page=0&sort[direction]=descending&payload[nid]=65350",
        "query_param" => "query[term]",
        "results_path" => "teaser",
        "title_field" => "teaser.title",
        "url_field" => "teaser.link.url",
        "url_prefix" => "https://www.bpb.de",
        "snippet_field" => "teaser.text",
    ])
}
