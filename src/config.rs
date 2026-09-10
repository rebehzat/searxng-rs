//! TOML configuration: global settings plus per-engine declarations.
//!
//! A configuration file is optional. Without one, the program uses
//! [`Config::builtin_defaults`], which enables the two bundled adapters
//! (`ddg_html` and `wikipedia`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::warn;

/// Per-engine configuration entry.
#[derive(Debug, Clone, Deserialize)]
pub struct EngineConfig {
    /// Adapter type: `duckduckgo_html`, `wikipedia`, ...
    #[serde(rename = "type")]
    pub engine_type: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Free-form adapter parameters (`language`, `region`, `weight`, ...).
    #[serde(flatten, default)]
    pub params: BTreeMap<String, toml::Value>,
}

impl EngineConfig {
    /// Convenience accessor for string parameters.
    pub fn string_param(&self, key: &str, default: &str) -> String {
        match self.params.get(key) {
            Some(toml::Value::String(s)) => s.clone(),
            _ => default.to_string(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Global settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Per-request HTTP timeout in seconds (overridable per invocation).
    pub timeout_secs: u64,
    /// Upper bound for in-flight engine requests.
    pub max_concurrent: usize,
    /// User-Agent sent with every request. Keep this honest: identify your
    /// client; this tool performs no anti-bot circumvention.
    pub user_agent: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout_secs: 10,
            max_concurrent: 4,
            user_agent: format!("searxng-rs/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Top-level configuration document.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub settings: Settings,
    /// Engine name (as used on the CLI) -> configuration.
    pub engines: BTreeMap<String, EngineConfig>,
}

impl Config {
    /// The configuration used when no file is present.
    pub fn builtin_defaults() -> Self {
        let definitions: Vec<(&str, &str, Vec<(&str, &str)>)> = vec![
            ("ddg_html", "duckduckgo_html", vec![("region", "wt-wt")]),
            ("wikipedia", "wikipedia", vec![("language", "en")]),
            (
                "openalex",
                "json_api",
                vec![
                    ("endpoint", "https://api.openalex.org/works"),
                    ("query_param", "search"),
                    ("limit_param", "per-page"),
                    ("results_path", "results"),
                    ("title_field", "title"),
                    ("url_field", "doi"),
                ],
            ),
            (
                "crossref",
                "json_api",
                vec![
                    ("endpoint", "https://api.crossref.org/works"),
                    ("query_param", "query.bibliographic"),
                    ("limit_param", "rows"),
                    ("results_path", "message.items"),
                    ("title_field", "title"),
                    ("url_field", "URL"),
                    ("snippet_field", "abstract"),
                ],
            ),
            (
                "semantic_scholar",
                "json_api",
                vec![
                    (
                        "endpoint",
                        "https://api.semanticscholar.org/graph/v1/paper/search?fields=title,url,abstract",
                    ),
                    ("query_param", "query"),
                    ("limit_param", "limit"),
                    ("results_path", "data"),
                    ("title_field", "title"),
                    ("url_field", "url"),
                    ("snippet_field", "abstract"),
                ],
            ),
            (
                "stackoverflow",
                "json_api",
                vec![
                    (
                        "endpoint",
                        "https://api.stackexchange.com/2.3/search/advanced?site=stackoverflow&order=desc&sort=relevance",
                    ),
                    ("query_param", "q"),
                    ("limit_param", "pagesize"),
                    ("results_path", "items"),
                    ("title_field", "title"),
                    ("url_field", "link"),
                    ("snippet_field", "excerpt"),
                ],
            ),
            (
                "openlibrary",
                "json_api",
                vec![
                    ("endpoint", "https://openlibrary.org/search.json"),
                    ("query_param", "q"),
                    ("limit_param", "limit"),
                    ("results_path", "docs"),
                    ("title_field", "title"),
                    ("url_field", "key"),
                    ("url_prefix", "https://openlibrary.org"),
                ],
            ),
            (
                "internet_archive",
                "json_api",
                vec![
                    (
                        "endpoint",
                        "https://archive.org/advancedsearch.php?output=json&fl[]=identifier&fl[]=title&fl[]=description",
                    ),
                    ("query_param", "q"),
                    ("limit_param", "rows"),
                    ("results_path", "response.docs"),
                    ("title_field", "title"),
                    ("url_field", "identifier"),
                    ("url_prefix", "https://archive.org/details"),
                    ("snippet_field", "description"),
                ],
            ),
            (
                "github_repositories",
                "json_api",
                vec![
                    ("endpoint", "https://api.github.com/search/repositories"),
                    ("query_param", "q"),
                    ("limit_param", "per_page"),
                    ("results_path", "items"),
                    ("title_field", "full_name"),
                    ("url_field", "html_url"),
                    ("snippet_field", "description"),
                ],
            ),
            (
                "hacker_news",
                "json_api",
                vec![
                    (
                        "endpoint",
                        "https://hn.algolia.com/api/v1/search?tags=story",
                    ),
                    ("query_param", "query"),
                    ("limit_param", "hitsPerPage"),
                    ("results_path", "hits"),
                    ("title_field", "title"),
                    ("url_field", "url"),
                    ("snippet_field", "story_text"),
                ],
            ),
            (
                "gitlab_projects",
                "json_api",
                vec![
                    ("endpoint", "https://gitlab.com/api/v4/projects"),
                    ("query_param", "search"),
                    ("limit_param", "per_page"),
                    ("title_field", "name_with_namespace"),
                    ("url_field", "web_url"),
                    ("snippet_field", "description"),
                ],
            ),
            (
                "npm_packages",
                "json_api",
                vec![
                    ("endpoint", "https://registry.npmjs.org/-/v1/search"),
                    ("query_param", "text"),
                    ("limit_param", "size"),
                    ("results_path", "objects"),
                    ("title_field", "package.name"),
                    ("url_field", "package.links.npm"),
                    ("snippet_field", "package.description"),
                ],
            ),
            (
                "google_books",
                "json_api",
                vec![
                    ("endpoint", "https://www.googleapis.com/books/v1/volumes"),
                    ("query_param", "q"),
                    ("limit_param", "maxResults"),
                    ("results_path", "items"),
                    ("title_field", "volumeInfo.title"),
                    ("url_field", "volumeInfo.infoLink"),
                    ("snippet_field", "volumeInfo.description"),
                ],
            ),
            (
                "tvmaze",
                "json_api",
                vec![
                    ("endpoint", "https://api.tvmaze.com/search/shows"),
                    ("query_param", "q"),
                    ("title_field", "show.name"),
                    ("url_field", "show.url"),
                    ("snippet_field", "show.summary"),
                ],
            ),
        ];
        let engines = definitions
            .into_iter()
            .map(|(name, ty, params)| {
                (
                    name.to_string(),
                    EngineConfig {
                        engine_type: ty.to_string(),
                        enabled: true,
                        params: params
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), toml::Value::from(v)))
                            .collect(),
                    },
                )
            })
            .collect();

        Self {
            settings: Settings::default(),
            engines,
        }
    }

    /// Parse configuration from a TOML string (used by tests and `--config -`).
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Load from an explicit path.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read config {}: {e}", path.display()))?;
        let cfg = Self::from_toml_str(&raw)
            .map_err(|e| anyhow::anyhow!("cannot parse config {}: {e}", path.display()))?;
        Ok(cfg)
    }

    /// Resolve the effective configuration: explicit `--config` path wins,
    /// then `./searxng-rs.toml`, then built-in defaults.
    pub fn resolve(explicit: Option<&PathBuf>) -> Self {
        if let Some(p) = explicit {
            match Self::load(p) {
                Ok(cfg) => return cfg,
                Err(e) => {
                    warn!("{e}");
                    return Self::builtin_defaults();
                }
            }
        }
        let local = PathBuf::from("searxng-rs.toml");
        if local.is_file() {
            match Self::load(&local) {
                Ok(cfg) => return cfg,
                Err(e) => warn!("{e}; falling back to built-in defaults"),
            }
        }
        Self::builtin_defaults()
    }

    /// Enabled engine configs, optionally filtered by explicit names.
    /// Unknown requested names are reported as an error string.
    pub fn select_engines(
        &self,
        requested: Option<&[String]>,
    ) -> (Vec<(String, EngineConfig)>, Vec<String>) {
        let mut selected = Vec::new();
        let mut unknown = Vec::new();
        match requested {
            Some(names) if !names.is_empty() => {
                for name in names {
                    match self.engines.get(name) {
                        Some(ec) if ec.enabled => {
                            selected.push((name.clone(), ec.clone()));
                        }
                        Some(_) => unknown.push(name.clone()), // disabled counts as unavailable
                        None => unknown.push(name.clone()),
                    }
                }
            }
            _ => {
                for (name, ec) in &self.engines {
                    if ec.enabled {
                        selected.push((name.clone(), ec.clone()));
                    }
                }
            }
        }
        (selected, unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[settings]
timeout_secs = 5
max_concurrent = 2
user_agent = "test-agent/1.0"

[engines.ddg_html]
type = "duckduckgo_html"
region = "de-de"

[engines.wikipedia]
type = "wikipedia"
language = "de"
enabled = true

[engines.off]
type = "wikipedia"
language = "fr"
enabled = false
"#;

    #[test]
    fn parses_sample_config() {
        let cfg = Config::from_toml_str(SAMPLE).unwrap();
        assert_eq!(cfg.settings.timeout_secs, 5);
        assert_eq!(cfg.settings.max_concurrent, 2);
        assert_eq!(cfg.engines.len(), 3);
        let wiki = &cfg.engines["wikipedia"];
        assert_eq!(wiki.engine_type, "wikipedia");
        assert_eq!(wiki.string_param("language", "en"), "de");
    }

    #[test]
    fn select_engines_filters_and_reports_unknown() {
        let cfg = Config::from_toml_str(SAMPLE).unwrap();

        let (sel, unknown) = cfg.select_engines(Some(&["ddg_html".into(), "nope".into()]));
        assert_eq!(sel.len(), 1);
        assert_eq!(unknown, vec!["nope".to_string()]);

        // Disabled engines are not selected by default...
        let (sel, _) = cfg.select_engines(None);
        assert_eq!(sel.len(), 2);
        // ...nor by explicit request.
        let (_, unknown) = cfg.select_engines(Some(&["off".into()]));
        assert_eq!(unknown, vec!["off".to_string()]);
    }

    #[test]
    fn builtin_defaults_contain_both_adapters() {
        let cfg = Config::builtin_defaults();
        assert!(cfg.engines.contains_key("ddg_html"));
        assert!(cfg.engines.contains_key("wikipedia"));
    }

    #[test]
    fn empty_file_is_valid() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.engines.is_empty());
        assert_eq!(cfg.settings.timeout_secs, 10);
    }
}
