//! One-file-per-engine built-in catalog.
//!
//! Each catalog entry owns its provider definition in a separate Rust file.
//! This keeps provider additions isolated: an engine PR can add exactly one
//! file without editing the central registry or conflicting with other engine
//! PRs.

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub name: &'static str,
    pub engine_type: &'static str,
    pub enabled: bool,
    pub params: Vec<(&'static str, &'static str)>,
}

#[macro_export]
macro_rules! engine_catalog_entry {
    ($name:literal, $engine_type:literal, [$($key:literal => $value:literal),* $(,)?]) => {
        $crate::engine_catalog_entry!($name, $engine_type, enabled = true, [$($key => $value),*])
    };
    ($name:literal, $engine_type:literal, enabled = $enabled:literal, [$($key:literal => $value:literal),* $(,)?]) => {
        $crate::engines::catalog::CatalogEntry {
            name: $name,
            engine_type: $engine_type,
            enabled: $enabled,
            params: vec![$(($key, $value)),*],
        }
    };
}

include!(concat!(env!("OUT_DIR"), "/catalog.rs"));

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_entries_have_unique_names() {
        let mut names = HashSet::new();
        for entry in definitions() {
            assert!(
                names.insert(entry.name),
                "duplicate engine catalog name '{}'",
                entry.name
            );
        }
    }

    fn entry(name: &str) -> CatalogEntry {
        definitions()
            .into_iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("missing catalog entry {name}"))
    }

    fn param(name: &str, key: &str) -> String {
        entry(name)
            .params
            .into_iter()
            .find_map(|(param, value)| (param == key).then(|| value.to_owned()))
            .unwrap_or_else(|| panic!("missing {key} parameter for {name}"))
    }

    #[test]
    fn safety_sensitive_catalog_entries_are_disabled_by_default() {
        // `unsplash` was removed from this list when it was enabled by default:
        // its endpoint answers an honest plain HTTP 200 and its field paths match
        // upstream. The remaining entries still ship opt-in, and each must keep a
        // comment in its own catalog file saying why.
        for name in ["deviantart", "openverse", "crates", "huggingface", "mwmbl"] {
            assert!(!entry(name).enabled, "{name} must be disabled by default");
        }
    }

    #[test]
    fn html_catalog_entries_use_provider_search_shapes() {
        let deviantart = entry("deviantart");
        assert_eq!(
            deviantart
                .params
                .iter()
                .find_map(|(key, value)| (*key == "result_selector").then_some(*value)),
            Some("div[data-testid=\"content_row\"]")
        );
        assert_eq!(param("deviantart", "link_selector"), "a[href][aria-label]");
        assert_eq!(param("deviantart", "title_attr"), "aria-label");
        assert_eq!(param("bandcamp", "endpoint"), "https://bandcamp.com/search");
        assert_eq!(param("bandcamp", "result_selector"), "li.searchresult");
        assert_eq!(param("bandcamp", "link_selector"), ".itemurl a");
    }

    #[test]
    fn json_catalog_entries_request_normalized_fields() {
        assert_eq!(param("dailymotion", "limit_param"), "limit");
        assert_eq!(param("dailymotion", "normalize_snippet_html"), "true");
        assert_eq!(param("dailymotion", "snippet_max_length"), "300");
        assert_eq!(param("mwmbl", "array_value_field"), "value");
        assert_eq!(param("peertube", "snippet_field"), "description");
        assert_eq!(param("sepiasearch", "snippet_field"), "description");
        assert_eq!(param("europepmc", "normalize_title_html"), "true");
        assert_eq!(param("europepmc", "normalize_snippet_html"), "true");
    }
}
