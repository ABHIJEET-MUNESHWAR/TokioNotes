use thiserror::Error;

/// Unified application-level error. Service layers convert into
/// transport-specific errors (GraphQL, HTTP) at the edges.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("dependency timed out: {0}")]
    Timeout(String),
    #[error("upstream failure: {0}")]
    Upstream(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type AppResult<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn internal<E: std::fmt::Display>(e: E) -> Self {
        AppError::Internal(e.to_string())
    }
    pub fn storage<E: std::fmt::Display>(e: E) -> Self {
        AppError::Storage(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_format() {
        assert_eq!(AppError::NotFound("x".into()).to_string(), "not found: x");
        assert!(matches!(AppError::internal("boom"), AppError::Internal(_)));
    }
}

