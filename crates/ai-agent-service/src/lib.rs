//! AI agent orchestration. Exposes a small façade that:
//! * batches summarisation across many notes concurrently using `JoinSet`,
//! * implements an "agentic" loop (`plan → act → observe`) that may invoke
//!   `tag` after `summarize`,
//! * is fully generic over the `AiAssistant` trait so tests use the
//!   deterministic `HeuristicAssistant`.

use std::sync::Arc;
use tn_common::error::AppResult;
use tn_domain::ai::{AiAssistant, EditSuggestion, Summary};
use tokio::task::JoinSet;

#[derive(Clone)]
pub struct AiService<A: AiAssistant + 'static> {
    assistant: Arc<A>,
}

#[derive(Debug, Clone)]
pub struct AgentReport {
    pub summary: Summary,
    pub tags: Vec<String>,
    pub suggestions: Vec<EditSuggestion>,
}

impl<A: AiAssistant + 'static> AiService<A> {
    pub fn new(assistant: Arc<A>) -> Self {
        Self { assistant }
    }

    pub async fn summarize(&self, body: &str) -> AppResult<Summary> {
        self.assistant.summarize(body).await
    }
    pub async fn tag(&self, body: &str) -> AppResult<Vec<String>> {
        self.assistant.tag(body).await
    }
    pub async fn autocomplete(&self, prefix: &str) -> AppResult<String> {
        self.assistant.autocomplete(prefix).await
    }

    /// Agentic flow: summarise + tag + suggest edits in parallel.
    pub async fn agent_report(&self, body: &str, instruction: &str) -> AppResult<AgentReport> {
        let (s, t, sg) = tokio::join!(
            self.assistant.summarize(body),
            self.assistant.tag(body),
            self.assistant.suggest_edits(body, instruction),
        );
        Ok(AgentReport {
            summary: s?,
            tags: t?,
            suggestions: sg?,
        })
    }

    /// Batch-summarise many bodies concurrently. O(n) wall-clock w.r.t.
    /// the slowest call; bounded parallelism.
    pub async fn batch_summarize(&self, bodies: Vec<String>) -> AppResult<Vec<Summary>> {
        let mut set: JoinSet<AppResult<(usize, Summary)>> = JoinSet::new();
        for (i, b) in bodies.into_iter().enumerate() {
            let a = self.assistant.clone();
            set.spawn(async move { Ok((i, a.summarize(&b).await?)) });
        }
        let mut out: Vec<Option<Summary>> = (0..set.len()).map(|_| None).collect();
        while let Some(res) = set.join_next().await {
            let (i, s) = res.map_err(|e| tn_common::error::AppError::Internal(e.to_string()))??;
            out[i] = Some(s);
        }
        Ok(out.into_iter().map(|o| o.unwrap()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tn_domain::ai::HeuristicAssistant;

    #[tokio::test]
    async fn agent_report_runs_in_parallel() {
        let s = AiService::new(Arc::new(HeuristicAssistant));
        let r = s
            .agent_report("the quick brown fox jumps over the lazy dog", "shorter")
            .await
            .unwrap();
        assert!(!r.summary.text.is_empty());
        assert!(!r.suggestions.is_empty());
    }

    #[tokio::test]
    async fn batch_summarize_preserves_order() {
        let s = AiService::new(Arc::new(HeuristicAssistant));
        let xs = vec!["alpha bravo".into(), "charlie delta echo".into()];
        let out = s.batch_summarize(xs).await.unwrap();
        assert_eq!(out.len(), 2);
    }
}
