// Wikimedia Commons media search through the public MediaWiki `query` API
// (searx `searx/engines/wikicommons.py`, upstream commit
// 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3; host from `wc_api_url`, which
// settings.yml does not override - the entry only sets `wc_search_type`).
//
// Upstream `request()` is a plain GET against `wc_api_url` with every argument
// except the offset baked into the query string. json_api can only append the
// search term and the page size, so the static arguments are transcribed
// verbatim into `endpoint` (`format`, `uselang`, `action`, `prop`,
// `generator`, `gsrnamespace`, `gsrprop`, `iiprop`, `iiurlheight`) and the two
// variable ones are mapped onto `query_param` (`gsrsearch`) and `limit_param`
// (`gsrlimit`, clamped to upstream `page_size` = 10 via `max_limit`).
//
// `formatversion=2` is the single added argument. At upstream's default
// formatversion=1 the generator returns `query.pages` as an *object* keyed by
// page id, which neither adapter can iterate; with formatversion=2 the same
// documented response is a plain array at `query.pages` (verified against the
// live API, no selectors or paths invented).
//
// `url_field` is the `imageinfo` array read with `array_value_field`, which
// reproduces upstream's `imageinfo[0]["descriptionurl"]`; `snippet` is the
// generator's search-match HTML, normalized as upstream's `html_to_text` does.
//
// Omitted because the adapters cannot express it (not guessed):
//   * the `filetype:` search filter upstream prepends to the query
//     (`bitmap|drawing` for the `image` engine this entry follows, `video`,
//     `audio`, `multimedia|office|archive|3d` for the other three upstream
//     instances): the adapter sends the query verbatim, so namespace 6 results
//     of any file type come back;
//   * `uselang`, which upstream derives from the searx locale - only the
//     upstream default `en` is sent;
//   * upstream's title cleanup (strip the `File:` prefix and the extension);
//   * the `images.html`/`Video`/`File`/default result templates and their
//     per-result metadata (img_src, thumbnail_src, width x height, filesize,
//     duration, filename);
//   * pagination: upstream `gsroffset` per `pageno`, so this is a first-page
//     subset only.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("wikicommons", "json_api", [
        "endpoint" => "https://commons.wikimedia.org/w/api.php?format=json&formatversion=2&uselang=en&action=query&prop=info%7Cimageinfo&generator=search&gsrnamespace=6&gsrprop=snippet&iiprop=url%7Csize%7Cmime&iiurlheight=180",
        "query_param" => "gsrsearch",
        "limit_param" => "gsrlimit",
        "max_limit" => "10",
        "results_path" => "query.pages",
        "title_field" => "title",
        "url_field" => "imageinfo",
        "array_value_field" => "descriptionurl",
        "snippet_field" => "snippet",
        "normalize_snippet_html" => "true",
    ])
}
