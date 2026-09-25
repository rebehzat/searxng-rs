pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("pdbe", "json_api", [
        "endpoint" => "https://www.ebi.ac.uk/pdbe/search/pdb/select?wt=json",
        "query_param" => "q",
        "results_path" => "response.docs",
        "title_field" => "title",
        "url_field" => "pdb_id",
        "url_prefix" => "https://www.ebi.ac.uk/pdbe/entry/pdb",
    ])
}
