//! Normalized data types shared by every engine and by the output layers.
//!
//! The whole program speaks in terms of [`SearchResult`] and
//! [`SearchResponse`]; engine adapters translate their raw payloads into
//! these structs so that merging, ranking and rendering stay uniform.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A single normalized search result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    /// Name of the engine that produced the result.
    pub engine: String,
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// 1-based position after merging across engines.
    pub rank: usize,
    /// Simple rank-based relevance score in (0.0, 1.0].
    pub score: f64,
    /// Engine-specific extras (e.g. Wikipedia description, language).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl SearchResult {
    /// Build a result with an explicitly supplied score (used by engines
    /// that know something about relevance, e.g. Wikipedia match order).
    pub fn with_metadata(
        engine: impl Into<String>,
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: Option<String>,
        metadata: HashMap<String, String>,
    ) -> Self {
        Self {
            engine: engine.into(),
            title: title.into(),
            url: url.into(),
            snippet,
            rank: 0,
            score: 0.0,
            metadata,
        }
    }

    /// Convenience constructor without metadata.
    pub fn new(
        engine: impl Into<String>,
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: Option<String>,
    ) -> Self {
        Self::with_metadata(engine, title, url, snippet, HashMap::new())
    }
}

/// Outcome of a single engine query (success or soft failure).
///
/// A failing engine does not fail the whole search; the error is surfaced
/// here so callers can see per-engine health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineStatus {
    pub name: String,
    pub ok: bool,
    pub count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Full response for one query: merged results plus per-engine status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub engines: Vec<EngineStatus>,
    /// Wall-clock duration of the whole (concurrent) search.
    pub elapsed_ms: u128,
}

/// Request frame for `--agent` JSONL mode (one JSON object per stdin line).
#[derive(Debug, Clone, Deserialize)]
pub struct AgentRequest {
    /// Opaque identifier echoed back to the caller (any JSON value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    pub query: String,
    /// Restrict to these engine names; defaults to all configured engines.
    #[serde(default)]
    pub engines: Option<Vec<String>>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Response frame for `--agent` JSONL mode (one JSON object per line).
#[derive(Debug, Serialize)]
pub struct AgentResponse<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<&'a serde_json::Value>,
    pub query: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub results: Vec<SearchResult>,
    pub engines: Vec<EngineStatus>,
    pub elapsed_ms: u128,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_result_roundtrips_through_json() {
        let mut meta = HashMap::new();
        meta.insert("lang".to_string(), "en".to_string());
        let r = SearchResult::with_metadata(
            "wikipedia",
            "Rust",
            "https://en.wikipedia.org/wiki/Rust",
            Some("A systems language".into()),
            meta,
        );

        let json = serde_json::to_string(&r).unwrap();
        let back: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn empty_fields_are_omitted_for_compact_frames() {
        let r = SearchResult::new("ddg_html", "T", "https://x.example", None);
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert!(v.get("snippet").is_none());
        assert!(v.get("metadata").is_none());
    }

    #[test]
    fn agent_request_accepts_optional_fields() {
        let req: AgentRequest = serde_json::from_str(r#"{"id": 7, "query": "rust"}"#).unwrap();
        assert_eq!(req.query, "rust");
        assert!(req.engines.is_none());
        assert!(req.limit.is_none());

        let req: AgentRequest =
            serde_json::from_str(r#"{"query": "rust", "engines": ["wikipedia"], "limit": 3}"#)
                .unwrap();
        assert_eq!(
            req.engines.as_deref().map(|v| v[0].as_str()),
            Some("wikipedia")
        );
        assert_eq!(req.limit, Some(3));
    }

    #[test]
    fn agent_request_rejects_missing_query() {
        let res: Result<AgentRequest, _> = serde_json::from_str(r#"{"limit": 3}"#);
        assert!(res.is_err());
    }
}
