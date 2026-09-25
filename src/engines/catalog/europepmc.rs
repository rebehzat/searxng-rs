pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("europepmc", "json_api", [
        "endpoint" => "https://www.ebi.ac.uk/europepmc/webservices/rest/search?format=json&resultType=core",
        "query_param" => "query",
        "limit_param" => "pageSize",
        "results_path" => "resultList.result",
        "title_field" => "title",
        "url_field" => "id",
        "url_template" => "https://europepmc.org/article/{source}/{value}",
        "url_source_field" => "source",
        "snippet_field" => "abstractText",
    ])
}
