//! Generative / Agentic AI ports. Implementations live in `infra` and the
//! `ai-agent-service`, but the domain only depends on this trait so business
//! logic stays decoupled from any vendor.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tn_common::error::AppResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub text: String,
    pub tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditSuggestion {
    pub anchor: usize,
    pub replacement: String,
    pub rationale: String,
}

#[async_trait]
pub trait AiAssistant: Send + Sync {
    async fn summarize(&self, body: &str) -> AppResult<Summary>;
    async fn autocomplete(&self, prefix: &str) -> AppResult<String>;
    async fn tag(&self, body: &str) -> AppResult<Vec<String>>;
    async fn suggest_edits(&self, body: &str, instruction: &str) -> AppResult<Vec<EditSuggestion>>;
}

/// A trivial deterministic implementation used in tests and as a fallback
/// when no LLM credentials are configured. Demonstrates the agentic
/// "tool-use" pattern: it can chain `summarize` → `tag` to produce both.
pub struct HeuristicAssistant;

#[async_trait]
impl AiAssistant for HeuristicAssistant {
    async fn summarize(&self, body: &str) -> AppResult<Summary> {
        let text = body.split_whitespace().take(20).collect::<Vec<_>>().join(" ");
        Ok(Summary { tokens: text.split_whitespace().count(), text })
    }
    async fn autocomplete(&self, prefix: &str) -> AppResult<String> {
        Ok(format!("{prefix}…"))
    }
    async fn tag(&self, body: &str) -> AppResult<Vec<String>> {
        let mut tags: Vec<String> = body
            .split_whitespace()
            .filter(|w| w.len() > 6)
            .take(5)
            .map(|w| w.to_lowercase())
            .collect();
        tags.sort();
        tags.dedup();
        Ok(tags)
    }
    async fn suggest_edits(&self, _body: &str, instruction: &str) -> AppResult<Vec<EditSuggestion>> {
        Ok(vec![EditSuggestion {
            anchor: 0,
            replacement: format!("// TODO: {instruction}\n"),
            rationale: "heuristic placeholder".into(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn heuristic_summary() {
        let a = HeuristicAssistant;
        let s = a.summarize("hello world from tokio notes").await.unwrap();
        assert!(s.text.starts_with("hello"));
    }
}

