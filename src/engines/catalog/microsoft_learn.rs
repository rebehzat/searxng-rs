pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("microsoft_learn", "json_api", enabled = false, [
        "endpoint" => "https://learn.microsoft.com/api/search?locale=en-us&scoringprofile=semantic-answers&facet=category&facet=products&facet=tags&%24top=10&%24skip=0&expandScope=true&includeQuestion=false&applyOperator=false&partnerId=LearnSite",
        "query_param" => "search",
        "results_path" => "results",
        "title_field" => "title",
        "url_field" => "url",
        "snippet_field" => "description",
    ])
}
