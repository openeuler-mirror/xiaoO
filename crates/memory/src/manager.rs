use crate::{
    DurableMemory, FactMemory, InstructionMemory, MemoryError, MemoryResult, MemorySnapshot,
    PromptHistoryEntry, RecallPacket, RecallQuery, SemanticMemoryStore, SemanticSearchQuery,
    SessionMemorySummary, TaskMemory, TokenUsageBaseline,
};
use agent_types::ChatMessage;

pub struct MemoryManager {
    snapshot: MemorySnapshot,
}

impl MemoryManager {
    pub fn new(session_id: impl Into<String>, updated_at: u64) -> MemoryResult<Self> {
        Ok(Self {
            snapshot: MemorySnapshot::new(session_id, updated_at, Vec::new())?,
        })
    }

    pub fn from_snapshot(snapshot: MemorySnapshot) -> Self {
        Self { snapshot }
    }

    pub fn snapshot(&self) -> &MemorySnapshot {
        &self.snapshot
    }

    pub fn sync_from_loop_state(
        &mut self,
        messages: &[ChatMessage],
        updated_at: u64,
    ) -> &MemorySnapshot {
        self.snapshot.sync_messages(messages, updated_at);
        &self.snapshot
    }

    pub fn add_instruction(
        &mut self,
        source: impl Into<String>,
        content: impl Into<String>,
    ) -> MemoryResult<()> {
        let source = source.into();
        if source.trim().is_empty() {
            return Err(MemoryError::EmptyInstructionSource);
        }

        self.snapshot.instructions.push(InstructionMemory {
            source,
            content: content.into(),
        });
        Ok(())
    }

    pub fn remember_fact(
        &mut self,
        key: impl Into<String>,
        content: impl Into<String>,
        recorded_at: u64,
    ) -> MemoryResult<()> {
        let key = key.into();
        if key.trim().is_empty() {
            return Err(MemoryError::EmptyFactKey);
        }

        self.snapshot.facts.push(FactMemory {
            key,
            content: content.into(),
            recorded_at,
        });
        Ok(())
    }

    pub fn set_current_task(
        &mut self,
        current_task: impl Into<String>,
        updated_at: u64,
    ) -> MemoryResult<()> {
        let current_task = current_task.into();
        if current_task.trim().is_empty() {
            return Err(MemoryError::EmptyTask);
        }

        let pending_steps = self
            .snapshot
            .task
            .as_ref()
            .map(|task| task.pending_steps.clone())
            .unwrap_or_default();

        self.snapshot.task = Some(TaskMemory {
            current_task,
            pending_steps,
            updated_at,
        });
        Ok(())
    }

    pub fn replace_pending_steps(
        &mut self,
        pending_steps: Vec<String>,
        updated_at: u64,
    ) -> MemoryResult<()> {
        let current_task = self
            .snapshot
            .task
            .as_ref()
            .map(|task| task.current_task.clone())
            .ok_or(MemoryError::EmptyTask)?;

        self.snapshot.task = Some(TaskMemory {
            current_task,
            pending_steps,
            updated_at,
        });
        Ok(())
    }

    pub fn record_prompt(&mut self, prompt: impl Into<String>, recorded_at: u64) {
        self.snapshot.prompt_history.push(PromptHistoryEntry {
            prompt: prompt.into(),
            recorded_at,
        });
    }

    pub fn record_usage_baseline(&mut self, baseline: TokenUsageBaseline) {
        self.snapshot.usage_baseline = Some(baseline);
    }

    pub fn attach_session_memory(&mut self, summary: SessionMemorySummary) -> MemoryResult<()> {
        if summary.session_id != self.snapshot.session_id {
            return Err(MemoryError::InvalidConfiguration {
                message: format!(
                    "session_id mismatch: snapshot has '{}' but summary has '{}'",
                    self.snapshot.session_id, summary.session_id,
                ),
            });
        }
        self.snapshot.session_memory = Some(summary);
        Ok(())
    }

    pub fn clear_session_memory(&mut self) {
        self.snapshot.session_memory = None;
    }

    pub fn build_recall(&self, query: &RecallQuery) -> RecallPacket {
        RecallPacket {
            instructions: self
                .snapshot
                .instructions
                .iter()
                .rev()
                .take(query.max_instruction_count)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            facts: self
                .snapshot
                .facts
                .iter()
                .rev()
                .take(query.max_fact_count)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            task: self.snapshot.task.clone(),
            prompt_history: self
                .snapshot
                .prompt_history
                .iter()
                .rev()
                .take(query.max_prompt_history_count)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            usage_baseline: self.snapshot.usage_baseline.clone(),
            session_memory: if query.include_session_memory {
                self.snapshot.session_memory.clone()
            } else {
                None
            },
            durable_memories: Vec::new(),
            semantic_results: Vec::new(),
        }
    }

    pub fn build_recall_with_durable(
        &self,
        query: &RecallQuery,
        durable_memories: &[DurableMemory],
    ) -> RecallPacket {
        let mut packet = self.build_recall(query);
        if query.include_durable_memory {
            packet.durable_memories = durable_memories.to_vec();
        }
        packet
    }

    /// Build recall with semantic search results from a SemanticMemoryStore.
    ///
    /// When `query.semantic_query` is Some, performs a hybrid vector+keyword
    /// search and attaches results to the packet. Falls back to basic recall
    /// when semantic_query is None.
    pub async fn build_recall_with_semantic(
        &self,
        query: &RecallQuery,
        durable_memories: &[DurableMemory],
        semantic_store: &dyn SemanticMemoryStore,
    ) -> MemoryResult<RecallPacket> {
        let mut packet = self.build_recall_with_durable(query, durable_memories);

        if let Some(ref semantic_query) = query.semantic_query {
            if !semantic_query.trim().is_empty() {
                let results = semantic_store
                    .search(&SemanticSearchQuery {
                        query_text: semantic_query.clone(),
                        limit: query.semantic_limit,
                        session_id: None,
                        kind_filter: None,
                    })
                    .await?;
                packet.semantic_results = results;
            }
        }

        Ok(packet)
    }
}
