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
