//! Error types for the crate.

use thiserror::Error;

/// Errors raised while fetching or parsing engine results.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("engine returned no parsable results (layout change or blocked)")]
    Parse,
}

pub type EngineResult<T> = std::result::Result<T, EngineError>;
