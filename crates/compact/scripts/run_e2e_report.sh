#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$CRATE_DIR/../.." && pwd)"
REPORT_DIR="$CRATE_DIR/reports"
REPORT_FILE="$REPORT_DIR/compact-memory-e2e-report.txt"

mkdir -p "$REPORT_DIR"

{
  printf 'compact + memory module report\n'
  printf 'generated_at=%s\n\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')"
  printf '[memory e2e scenarios]\n'
  cat "$REPO_DIR/crates/memory/tests/README.md"
  printf '\n\n'
  printf '[memory tests]\n'
  cargo test --manifest-path "$REPO_DIR/crates/memory/Cargo.toml" -- --nocapture
  printf '\n[compact e2e scenarios]\n'
  cat "$REPO_DIR/crates/compact/tests/README.md"
  printf '\n\n'
  printf '\n[compact tests]\n'
  cargo test --manifest-path "$REPO_DIR/crates/compact/Cargo.toml" -- --nocapture
  printf '\n[live llm tests]\n'
  if [[ -n "${OPENROUTER_API_KEY:-}" && -n "${OPENROUTER_API_BASE:-}" && -n "${OPENROUTER_MODEL:-}" && -n "${LIVE_LLM_MAX_TOKENS:-}" && -n "${LIVE_LLM_TEMPERATURE:-}" ]]; then
    cargo test \
      --manifest-path "$REPO_DIR/crates/compact/Cargo.toml" \
      live_llm_provider_drives_memory_and_compact_summaries \
      -- --ignored --nocapture
  else
    printf 'skipped: set OPENROUTER_API_KEY, OPENROUTER_API_BASE, OPENROUTER_MODEL, LIVE_LLM_MAX_TOKENS, LIVE_LLM_TEMPERATURE to include the real-provider smoke test.\n'
  fi
} | tee "$REPORT_FILE"
