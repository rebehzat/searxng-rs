// OpenStreetMap places via the public Nominatim `search` endpoint
// (searx `searx/engines/openstreetmap.py`, upstream commit
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3).
//
// First-page normalized subset: the endpoint is a plain top-level JSON array
// (no wrapper object, so no `results_path`), and the per-place link is rebuilt
// from explicit `osm_type`/`osm_id` fields as in upstream
// `get_url_osm_geojson`. `display_name` is the adapter's unconditional title
// fallback, rather than upstream's category-specific named-place title.
// Nominatim sends no explicit page size upstream, so no limit parameter is
// added here either; the adapter still truncates to the caller's limit
// client-side.
//
// Deliberately not represented (the current adapters cannot express it, and
// guessing would be inaccurate): the `A` -> `B` "Show route in map" answer, the
// `map.html` result template with address/geojson/thumbnail/links/typed tag
// data, the localized OSM key/tag labels, the Wikidata SPARQL image/label
// lookup, and the latitude/longitude map URL that upstream uses when a place
// carries no `osm_id` (such places are dropped here instead of being given a
// synthesized link). Places without `osm_id`/`osm_type` are therefore skipped.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("openstreetmap", "json_api", [
        "endpoint" => "https://nominatim.openstreetmap.org/search?polygon_geojson=1&format=jsonv2&addressdetails=1&extratags=1&dedupe=1",
        "query_param" => "q",
        "title_field" => "display_name",
        "url_field" => "osm_id",
        "url_source_field" => "osm_type",
        "url_template" => "https://openstreetmap.org/{source}/{value}",
    ])
}
