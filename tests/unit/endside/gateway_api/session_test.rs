use super::SessionGateway;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use xiaoo_shared::gateway::memory_automation::{
    CompletedTurnIngest, MemoryAutomationError, RecallMemory, TurnMemoryContext,
};
use xiaoo_shared::gateway::TurnMemoryAutomation;

struct ClosingAutomation(AtomicBool);

#[async_trait]
impl TurnMemoryAutomation for ClosingAutomation {
    async fn recall(
        &self,
        _context: &TurnMemoryContext,
    ) -> Result<Vec<RecallMemory>, MemoryAutomationError> {
        Ok(Vec::new())
    }

    async fn enqueue_ingest(
        &self,
        _ingest: CompletedTurnIngest,
    ) -> Result<(), MemoryAutomationError> {
        Ok(())
    }

    fn recall_token_budget(&self) -> usize {
        0
    }

    async fn close(&self) -> Result<(), MemoryAutomationError> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn close_all_sessions_closes_cached_memory_automation() {
    let gateway = SessionGateway::new();
    let automation = Arc::new(ClosingAutomation(AtomicBool::new(false)));
    *gateway.memory_automation.lock().await =
        Some(Some(automation.clone() as Arc<dyn TurnMemoryAutomation>));

    gateway.close_all_sessions().await;

    assert!(automation.0.load(Ordering::SeqCst));
    assert!(gateway.memory_automation.lock().await.is_none());
}
