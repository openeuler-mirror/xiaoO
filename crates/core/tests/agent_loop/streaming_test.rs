/// Microbenchmark comparing old (full-text clone) vs new (delta) streaming paths.
///
/// Run with `--nocapture` to see per-chunk timing:
///   cargo test stream_chunk_bench -- --nocapture
#[test]
fn stream_chunk_bench() {
    use std::sync::Mutex;
    use std::time::Instant;

    // --- Old path sink: default supports_message_delta() = false ---
    #[derive(Default)]
    struct OldPathSink {
        last_text: Mutex<String>,
        last_reasoning: Mutex<String>,
    }
    impl LoopEventSink for OldPathSink {
        fn on_turn_start(&self, _: &AgentId, _: u32) {}
        fn on_assistant_message(&self, _: &AgentId, text: &str) {
            *self.last_text.lock().unwrap() = text.to_string();
        }
        fn on_assistant_reasoning(&self, _: &AgentId, text: &str) {
            *self.last_reasoning.lock().unwrap() = text.to_string();
        }
        fn on_tool_result(&self, _: &AgentId, _: &ToolResultEvent) {}
        fn on_loop_end(&self, _: &AgentId, _: &LoopEndSummary) {}
    }

    // --- New path sink: supports_message_delta() = true ---
    // text/reasoning are accumulated independently so the bench can assert
    // the delta path matches the full-snapshot path for each stream.
    #[derive(Default)]
    struct NewPathSink {
        text: Mutex<String>,
        reasoning: Mutex<String>,
    }
    impl LoopEventSink for NewPathSink {
        fn on_turn_start(&self, _: &AgentId, _: u32) {}
        fn on_assistant_message(&self, _: &AgentId, text: &str) {
            // Full-snapshot fallback: used when delta support is unchecked
            // or when secrets force a snapshot. Replace, not append.
            *self.text.lock().unwrap() = text.to_string();
        }
        fn on_assistant_message_delta(&self, _: &AgentId, delta: &str) {
            self.text.lock().unwrap().push_str(delta);
        }
        fn on_assistant_reasoning(&self, _: &AgentId, text: &str) {
            *self.reasoning.lock().unwrap() = text.to_string();
        }
        fn on_assistant_reasoning_delta(&self, _: &AgentId, delta: &str) {
            self.reasoning.lock().unwrap().push_str(delta);
        }
        fn supports_message_delta(&self) -> bool {
            true
        }
        fn on_tool_result(&self, _: &AgentId, _: &ToolResultEvent) {}
        fn on_loop_end(&self, _: &AgentId, _: &LoopEndSummary) {}
    }

    // Generate test chunks: 500 chunks × 80 bytes = 40K response
    let chunk_base = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. ";
    let num_chunks = 500;
    let chunks: Vec<StreamChunk> = (0..num_chunks)
        .map(|i| {
            let offset = (i * 7) % (chunk_base.len() - 40);
            let text = &chunk_base[offset..offset + 40];
            // Independent reasoning stream: different slice of the same base
            // so the two streams carry distinct content and can be verified
            // separately.
            let reasoning_offset = (i * 11) % (chunk_base.len() - 40);
            let reasoning = &chunk_base[reasoning_offset..reasoning_offset + 40];
            StreamChunk {
                delta_text: Some(text.to_string()),
                delta_reasoning: Some(reasoning.to_string()),
                delta_tool_call: None,
            }
        })
        .collect();
    let total_chars: usize = chunks
        .iter()
        .filter_map(|c| c.delta_text.as_ref())
        .map(|t| t.len())
        .sum();
    let agent_id = AgentId("bench".to_string());

    // ---- Warmup ----
    let old_sink = OldPathSink::default();
    let new_sink = NewPathSink::default();
    let old_text = Mutex::new(String::new());
    let old_reasoning = Mutex::new(String::new());
    let old_text_emit = Mutex::new(StreamEmitState::default());
    let old_reasoning_emit = Mutex::new(StreamEmitState::default());
    for _ in 0..5 {
        for chunk in &chunks {
            stream_assistant_chunk(
                Some(&old_sink),
                &agent_id,
                &old_text,
                &old_reasoning,
                &old_text_emit,
                &old_reasoning_emit,
                chunk.clone(),
                &[],
            );
        }
    }
    let new_text = Mutex::new(String::new());
    let new_reasoning = Mutex::new(String::new());
    let new_text_emit = Mutex::new(StreamEmitState::default());
    let new_reasoning_emit = Mutex::new(StreamEmitState::default());
    for _ in 0..5 {
        for chunk in &chunks {
            stream_assistant_chunk(
                Some(&new_sink),
                &agent_id,
                &new_text,
                &new_reasoning,
                &new_text_emit,
                &new_reasoning_emit,
                chunk.clone(),
                &[],
            );
        }
    }

    // ---- Measure old path (full-text clone) ----
    // Use fresh sinks so old/new paths end in a comparable state (1× full
    // text each); the warmup sinks accumulated 5 rounds of delta appends
    // and are not directly comparable. Fresh emit-state mutexes too, so the
    // throttle's `last_emit_len` (left at ~100K after warmup) can't suppress
    // the measurement emits (each 40-char chunk must clear the 32-char delta
    // gate from a zero baseline).
    let old_sink = OldPathSink::default();
    let old_text = Mutex::new(String::new());
    let old_reasoning = Mutex::new(String::new());
    let old_text_emit = Mutex::new(StreamEmitState::default());
    let old_reasoning_emit = Mutex::new(StreamEmitState::default());
    let old_start = Instant::now();
    for chunk in &chunks {
        stream_assistant_chunk(
            Some(&old_sink),
            &agent_id,
            &old_text,
            &old_reasoning,
            &old_text_emit,
            &old_reasoning_emit,
            chunk.clone(),
            &[],
        );
    }
    let old_elapsed = old_start.elapsed();

    // ---- Measure new path (delta) ----
    let new_sink = NewPathSink::default();
    let new_text = Mutex::new(String::new());
    let new_reasoning = Mutex::new(String::new());
    let new_text_emit = Mutex::new(StreamEmitState::default());
    let new_reasoning_emit = Mutex::new(StreamEmitState::default());
    let new_start = Instant::now();
    for chunk in &chunks {
        stream_assistant_chunk(
            Some(&new_sink),
            &agent_id,
            &new_text,
            &new_reasoning,
            &new_text_emit,
            &new_reasoning_emit,
            chunk.clone(),
            &[],
        );
    }
    let new_elapsed = new_start.elapsed();

    // Correctness: the delta path must accumulate the same final output as
    // the full-snapshot path for both text and reasoning. This guards
    // against future regressions in stream_assistant_chunk's streaming
    // semantics (empty deltas, length mismatch, text/reasoning cross-talk).
    assert_eq!(
        old_sink.last_text.lock().unwrap().as_str(),
        new_sink.text.lock().unwrap().as_str(),
        "delta path text diverges from full-snapshot path"
    );
    assert_eq!(
        old_sink.last_reasoning.lock().unwrap().as_str(),
        new_sink.reasoning.lock().unwrap().as_str(),
        "delta path reasoning diverges from full-snapshot path"
    );

    let old_us = old_elapsed.as_micros();
    let new_us = new_elapsed.as_micros();
    let ratio = if new_us > 0 {
        old_us as f64 / new_us as f64
    } else {
        f64::INFINITY
    };

    eprintln!();
    eprintln!("=== stream_assistant_chunk microbenchmark ===");
    eprintln!("  Chunks: {num_chunks} × ~40 chars = ~{total_chars} chars total response");
    eprintln!(
        "  Old path (full-text clone): {old_us:>8} µs  ({:.2} µs/chunk)",
        old_us as f64 / num_chunks as f64
    );
    eprintln!(
        "  New path (delta):          {new_us:>8} µs  ({:.2} µs/chunk)",
        new_us as f64 / num_chunks as f64
    );
    eprintln!("  Speedup: {ratio:.1}× faster");
    eprintln!();

    // Assert that new path is at least as fast (not significantly slower).
    // Allow 20% tolerance for noise.
    assert!(
        new_us <= old_us + old_us / 5,
        "New path ({new_us}µs) should not be significantly slower than old path ({old_us}µs)"
    );
}

/// A delta-capable sink that accumulates text and reasoning independently,
/// so tests can assert on each stream in isolation. `on_assistant_message`
/// / `on_assistant_reasoning` replace the accumulated string (the full
/// snapshot fallback path), while the `_delta` variants append.
#[derive(Default)]
struct DeltaSink {
    text: Mutex<String>,
    reasoning: Mutex<String>,
}
impl LoopEventSink for DeltaSink {
    fn on_turn_start(&self, _: &AgentId, _: u32) {}
    fn on_assistant_message(&self, _: &AgentId, text: &str) {
        *self.text.lock().unwrap() = text.to_string();
    }
    fn on_assistant_message_delta(&self, _: &AgentId, delta: &str) {
        self.text.lock().unwrap().push_str(delta);
    }
    fn on_assistant_reasoning(&self, _: &AgentId, text: &str) {
        *self.reasoning.lock().unwrap() = text.to_string();
    }
    fn on_assistant_reasoning_delta(&self, _: &AgentId, delta: &str) {
        self.reasoning.lock().unwrap().push_str(delta);
    }
    fn supports_message_delta(&self) -> bool {
        true
    }
    fn on_tool_result(&self, _: &AgentId, _: &ToolResultEvent) {}
    fn on_loop_end(&self, _: &AgentId, _: &LoopEndSummary) {}
}

/// Regression: a secret split across two text chunks must not leak its
/// already-emitted prefix. When secrets are non-empty the delta fast path
/// is skipped (`supports_message_delta() && secrets.is_empty()` is false),
/// so each emit goes through the full-snapshot fallback where
/// `on_assistant_message` *replaces* the whole string with the filtered
/// snapshot — the append-only delta leak (`"Hello passRET> world"`) cannot
/// occur. Chunks are padded past `STREAM_EMIT_MIN_DELTA_CHARS` (32) so the
/// throttle actually emits on both chunks; otherwise the assertions would
/// pass vacuously on an empty sink.
#[test]
fn stream_secret_split_across_text_chunk_boundary_does_not_leak() {
    let sink = DeltaSink::default();
    let streamed_text = Mutex::new(String::new());
    let streamed_reasoning = Mutex::new(String::new());
    let last_text_emit = Mutex::new(StreamEmitState::default());
    let last_reasoning_emit = Mutex::new(StreamEmitState::default());
    let agent_id = AgentId("test".to_string());
    let secrets = vec!["password123".to_string()];

    let chunks = [
        StreamChunk {
            delta_text: Some("Some padding text before Hello pass".to_string()),
            delta_reasoning: None,
            delta_tool_call: None,
        },
        StreamChunk {
            delta_text: Some("word123 plus some trailing padding!!".to_string()),
            delta_reasoning: None,
            delta_tool_call: None,
        },
    ];

    for chunk in chunks {
        stream_assistant_chunk(
            Some(&sink),
            &agent_id,
            &streamed_text,
            &streamed_reasoning,
            &last_text_emit,
            &last_reasoning_emit,
            chunk,
            &secrets,
        );
        // The full secret must never be present in the accumulated text.
        assert!(
            !sink.text.lock().unwrap().contains("password123"),
            "secret leaked after chunk"
        );
    }

    assert_eq!(
        sink.text.lock().unwrap().as_str(),
        "Some padding text before Hello <SECRET> plus some trailing padding!!"
    );
}

/// Regression: symmetric to the text case — a secret split across two
/// reasoning chunks must not leak via the reasoning delta path. Same
/// padding rationale as the text test (clear the 32-char throttle gate).
#[test]
fn stream_secret_split_across_reasoning_chunk_boundary_does_not_leak() {
    let sink = DeltaSink::default();
    let streamed_text = Mutex::new(String::new());
    let streamed_reasoning = Mutex::new(String::new());
    let last_text_emit = Mutex::new(StreamEmitState::default());
    let last_reasoning_emit = Mutex::new(StreamEmitState::default());
    let agent_id = AgentId("test".to_string());
    let secrets = vec!["password123".to_string()];

    let chunks = [
        StreamChunk {
            delta_text: None,
            delta_reasoning: Some("Some padding text before Thinking pass".to_string()),
            delta_tool_call: None,
        },
        StreamChunk {
            delta_text: None,
            delta_reasoning: Some("word123 plus some trailing padding!!".to_string()),
            delta_tool_call: None,
        },
    ];

    for chunk in chunks {
        stream_assistant_chunk(
            Some(&sink),
            &agent_id,
            &streamed_text,
            &streamed_reasoning,
            &last_text_emit,
            &last_reasoning_emit,
            chunk,
            &secrets,
        );
        assert!(
            !sink.reasoning.lock().unwrap().contains("password123"),
            "secret leaked after reasoning chunk"
        );
    }

    assert_eq!(
        sink.reasoning.lock().unwrap().as_str(),
        "Some padding text before Thinking <SECRET> plus some trailing padding!!"
    );
}

/// The daemon/SSE path inherits `FeatureFlags::default()` (it never
/// overrides `redact_secrets_display`), so the default must stay `true`
/// to keep the remote stream always-redacted. The local TUI overrides
/// this from its own config (default `false`); see the endside crate's
/// `TuiConfig` test. Guarding this default here prevents a future change
/// from silently disabling remote redaction.
#[test]
fn feature_flags_default_redacts_secrets_for_daemon() {
    assert!(
        FeatureFlags::default().redact_secrets_display,
        "FeatureFlags::default() must redact secrets (daemon inherits this)"
    );
}
