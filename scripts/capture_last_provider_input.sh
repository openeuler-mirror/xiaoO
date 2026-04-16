#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROVIDER="openrouter"
MODEL="${XIAOO_CAPTURE_MODEL:-z-ai/glm-5}"
HOST="127.0.0.1"
PORT="${XIAOO_CAPTURE_PORT:-$((20000 + RANDOM % 20000))}"
OUTPUT_DIR=""
DAEMON_PID=""

usage() {
  cat <<'EOF'
Usage:
  scripts/capture_last_provider_input.sh [--provider <provider>] [--model <model>] [--output-dir <dir>] [--port <port>]

What it does:
  1. Starts the xiaoo HTTP daemon with stdout trace enabled.
  2. Sends 3 real user turns to the same session via /api/v1/chat.
  3. Extracts the last LLM call's effective_request from trace logs.
  4. Reconstructs the final OpenAI-compatible request body sent to the provider.

Defaults:
  provider: openrouter
  model:    z-ai/glm-5

Notes:
  - For openrouter, OPENROUTER_API_KEY must already be set in the environment.
  - The script prints the final provider request body, but never prints auth headers.
EOF
}

cleanup() {
  if [[ -n "${DAEMON_PID}" ]] && kill -0 "${DAEMON_PID}" 2>/dev/null; then
    kill "${DAEMON_PID}" 2>/dev/null || true
    wait "${DAEMON_PID}" 2>/dev/null || true
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --provider)
      PROVIDER="${2:-}"
      shift 2
      ;;
    --model)
      MODEL="${2:-}"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="${2:-}"
      shift 2
      ;;
    --port)
      PORT="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "${PROVIDER}" == "openrouter" && -z "${OPENROUTER_API_KEY:-}" ]]; then
  echo "OPENROUTER_API_KEY is not set. Export it before running this script." >&2
  exit 1
fi

mkdir -p "${ROOT_DIR}/target"
if [[ -z "${OUTPUT_DIR}" ]]; then
  OUTPUT_DIR="$(mktemp -d "${ROOT_DIR}/target/provider-input-capture.XXXXXX")"
else
  mkdir -p "${OUTPUT_DIR}"
fi

trap cleanup EXIT

README_PATH="${ROOT_DIR}/README.md"
CHANNEL="capture"
CHANNEL_INSTANCE_ID="capture-http"
SENDER_ID="capture-user"
CONVERSATION_ID="conv-$(uuidgen | tr '[:upper:]' '[:lower:]')"

TURN1_TEXT=$(cat <<EOF
第一轮：请你必须调用一次 file_read 工具，读取 ${README_PATH}，找出第一个 Markdown 一级标题。回复只输出标题文本本身，不要解释。
EOF
)

TURN2_TEXT=$(cat <<'EOF'
第二轮：请你必须调用一次 count_text_length 工具，统计我们上一轮已经得到的标题字符数。回复只输出数字，不要解释。
EOF
)

TURN3_TEXT=$(cat <<'EOF'
第三轮：现在根据我们前两轮已经确认的信息，用一句话回答，格式必须严格为：标题: <标题>；字符数: <数字>。不要调用任何工具。
EOF
)

DAEMON_CONFIG_FILE="${OUTPUT_DIR}/daemon-config.toml"
DAEMON_LOG_FILE="${OUTPUT_DIR}/daemon.log"
TRACE_JSONL_FILE="${OUTPUT_DIR}/trace-lines.jsonl"
TRANSCRIPT_FILE="${OUTPUT_DIR}/transcript.md"
TURN1_REQUEST_FILE="${OUTPUT_DIR}/turn1.request.json"
TURN1_RESPONSE_FILE="${OUTPUT_DIR}/turn1.response.json"
TURN2_REQUEST_FILE="${OUTPUT_DIR}/turn2.request.json"
TURN2_RESPONSE_FILE="${OUTPUT_DIR}/turn2.response.json"
TURN3_REQUEST_FILE="${OUTPUT_DIR}/turn3.request.json"
TURN3_RESPONSE_FILE="${OUTPUT_DIR}/turn3.response.json"
LAST_EFFECTIVE_REQUEST_FILE="${OUTPUT_DIR}/last_effective_request.json"
LAST_PROVIDER_REQUEST_FILE="${OUTPUT_DIR}/last_provider_request_body.json"
LLM_CALL_COUNT_FILE="${OUTPUT_DIR}/llm_call_count.txt"

cat >"${DAEMON_CONFIG_FILE}" <<EOF
[llm]
provider = "${PROVIDER}"
model = "${MODEL}"
api_key_env = "OPENROUTER_API_KEY"
context_window = 128000

[trace]
storage_backend = "stdout"
EOF

wait_for_health() {
  local attempts=0
  while [[ ${attempts} -lt 120 ]]; do
    if curl -fsS "http://${HOST}:${PORT}/api/v1/health" >/dev/null 2>&1; then
      return 0
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  return 1
}

write_turn_request() {
  local text="$1"
  local message_id="$2"
  local request_file="$3"

  jq -n \
    --arg text "${text}" \
    --arg channel "${CHANNEL}" \
    --arg channel_instance_id "${CHANNEL_INSTANCE_ID}" \
    --arg sender_id "${SENDER_ID}" \
    --arg conversation_id "${CONVERSATION_ID}" \
    --arg message_id "${message_id}" \
    '{
      text: $text,
      channel: $channel,
      channel_instance_id: $channel_instance_id,
      sender_id: $sender_id,
      conversation_id: $conversation_id,
      message_id: $message_id
    }' >"${request_file}"
}

send_turn() {
  local request_file="$1"
  local response_file="$2"
  curl -fsS \
    -H 'Content-Type: application/json' \
    --data @"${request_file}" \
    "http://${HOST}:${PORT}/api/v1/chat" >"${response_file}"
}

(
  cd "${ROOT_DIR}"
  cargo run -q -p xiaoo-app --bin xiaoo-app -- \
    daemon \
    --config "${DAEMON_CONFIG_FILE}" \
    --host "${HOST}" \
    --port "${PORT}"
) >"${DAEMON_LOG_FILE}" 2>&1 &
DAEMON_PID=$!

if ! wait_for_health; then
  echo "Daemon did not become healthy. See ${DAEMON_LOG_FILE}" >&2
  exit 1
fi

write_turn_request "${TURN1_TEXT}" "turn-1" "${TURN1_REQUEST_FILE}"
write_turn_request "${TURN2_TEXT}" "turn-2" "${TURN2_REQUEST_FILE}"
write_turn_request "${TURN3_TEXT}" "turn-3" "${TURN3_REQUEST_FILE}"

send_turn "${TURN1_REQUEST_FILE}" "${TURN1_RESPONSE_FILE}"
send_turn "${TURN2_REQUEST_FILE}" "${TURN2_RESPONSE_FILE}"
send_turn "${TURN3_REQUEST_FILE}" "${TURN3_RESPONSE_FILE}"

TURN1_SESSION_ID="$(jq -r '.session_id' "${TURN1_RESPONSE_FILE}")"
TURN2_SESSION_ID="$(jq -r '.session_id' "${TURN2_RESPONSE_FILE}")"
TURN3_SESSION_ID="$(jq -r '.session_id' "${TURN3_RESPONSE_FILE}")"

if [[ "${TURN1_SESSION_ID}" != "${TURN2_SESSION_ID}" || "${TURN2_SESSION_ID}" != "${TURN3_SESSION_ID}" ]]; then
  echo "Session reuse failed across turns." >&2
  exit 1
fi

cleanup
DAEMON_PID=""

grep '"record_type":"trace_span"' "${DAEMON_LOG_FILE}" >"${TRACE_JSONL_FILE}" || true

if [[ ! -s "${TRACE_JSONL_FILE}" ]]; then
  echo "No trace span records were captured. See ${DAEMON_LOG_FILE}" >&2
  exit 1
fi

jq -s '
  map(
    select(
      .record_type == "trace_span"
      and .span.span_kind == "LlmCall"
      and (.span.fields.effective_request? != null)
    )
  )
  | map(.span.span_id)
  | unique
  | length
' "${TRACE_JSONL_FILE}" >"${LLM_CALL_COUNT_FILE}"

jq -s '
  map(
    select(
      .record_type == "trace_span"
      and .span.span_kind == "LlmCall"
      and (.span.fields.effective_request? != null)
    )
  )
  | last
  | .span.fields.effective_request
' "${TRACE_JSONL_FILE}" >"${LAST_EFFECTIVE_REQUEST_FILE}"

if [[ "$(jq -r 'type' "${LAST_EFFECTIVE_REQUEST_FILE}")" == "null" ]]; then
  echo "Failed to extract the last effective_request. See ${DAEMON_LOG_FILE}" >&2
  exit 1
fi

jq --arg model "${MODEL}" '
  def wire_message:
    reduce .blocks[] as $block (
      { role: .role };
      if $block.type == "text" then
        .content = $block.text
      elif $block.type == "tool_use" then
        .tool_calls = ((.tool_calls // []) + [{
          id: $block.call_id,
          type: "function",
          function: {
            name: $block.tool_name,
            arguments: ($block.input | tojson)
          }
        }])
      elif $block.type == "tool_result" then
        .tool_call_id = $block.call_id
        | .content = $block.output
      elif $block.type == "image" or $block.type == "document" then
        .content = $block.description
      else
        .
      end
    );

  def wire_tools:
    .tools
    | map({
        type: "function",
        function: {
          name: .name,
          description: .description,
          parameters: .parameters
        }
      });

  def wire_tool_choice:
    if .tool_choice == "auto" or .tool_choice == "required" or .tool_choice == "none" then
      .tool_choice
    elif (.tool_choice | type) == "object" and .tool_choice.specific? != null then
      {
        type: "function",
        function: {
          name: .tool_choice.specific
        }
      }
    else
      null
    end;

  def wire_response_format:
    if .response_format == "json_object" then
      { type: "json_object" }
    elif (.response_format | type) == "object" and .response_format.json_schema? != null then
      {
        type: "json_schema",
        json_schema: {
          name: (
            (.response_format.json_schema.name // "")
            | if . == "" then "response" else . end
          ),
          strict: true,
          schema: .response_format.json_schema.schema
        }
      }
    else
      null
    end;

  {
    model: $model,
    messages: (.messages | map(wire_message)),
    stream: true
  }
  + (
      if .temperature != null then
        { temperature: ((.temperature * 100 | round) / 100) }
      else
        {}
      end
    )
  + (
      if .max_tokens != null then
        { max_tokens: .max_tokens }
      else
        {}
      end
    )
  + (
      if (.tools | length) > 0 then
        { tools: wire_tools }
      else
        {}
      end
    )
  + (
      if (.tools | length) > 0 and (wire_tool_choice != null) then
        { tool_choice: wire_tool_choice }
      else
        {}
      end
    )
  + (
      if wire_response_format != null then
        { response_format: wire_response_format }
      else
        {}
      end
    )
' "${LAST_EFFECTIVE_REQUEST_FILE}" >"${LAST_PROVIDER_REQUEST_FILE}"

cat >"${TRANSCRIPT_FILE}" <<EOF
# Real Three-Turn Conversation

## Turn 1 User
${TURN1_TEXT}

## Turn 1 Assistant
$(jq -r '.reply' "${TURN1_RESPONSE_FILE}")

## Turn 2 User
${TURN2_TEXT}

## Turn 2 Assistant
$(jq -r '.reply' "${TURN2_RESPONSE_FILE}")

## Turn 3 User
${TURN3_TEXT}

## Turn 3 Assistant
$(jq -r '.reply' "${TURN3_RESPONSE_FILE}")
EOF

echo "Artifacts written to: ${OUTPUT_DIR}"
echo "Session ID: ${TURN3_SESSION_ID}"
echo "Detected LLM calls: $(cat "${LLM_CALL_COUNT_FILE}")"
echo
echo "Turn replies:"
echo "  Turn 1: $(jq -r '.reply' "${TURN1_RESPONSE_FILE}")"
echo "  Turn 2: $(jq -r '.reply' "${TURN2_RESPONSE_FILE}")"
echo "  Turn 3: $(jq -r '.reply' "${TURN3_RESPONSE_FILE}")"
echo
echo "Last effective request:"
echo "  ${LAST_EFFECTIVE_REQUEST_FILE}"
echo "Last provider request body:"
echo "  ${LAST_PROVIDER_REQUEST_FILE}"
echo
jq '.' "${LAST_PROVIDER_REQUEST_FILE}"
