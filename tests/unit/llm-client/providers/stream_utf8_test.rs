use super::*;
use crate::providers::{
    AnthropicProvider, GeminiProvider, OllamaProvider, OpenAiFamilyAuthStyle, OpenAiFamilyProvider,
};
use agent_contracts::LlmProvider;
use agent_llm::{ChatMessageExt, LlmRequestExt};
use agent_types::{ChatMessage, LlmRequest, StopReason};
use std::sync::Mutex;

fn decode(chunks: &[&[u8]]) -> String {
    let mut decoder = StreamUtf8Decoder::default();
    let mut output = String::new();
    for chunk in chunks {
        decoder.push(chunk, &mut output);
        assert!(decoder.pending.len() <= 3);
    }
    decoder.finish(&mut output);
    output
}

#[test]
fn legal_utf8_survives_every_pair_of_byte_boundaries() {
    let text = "ASCII é¢你好€🙂𐍈\r\n再见";
    let bytes = text.as_bytes();
    for first in 0..=bytes.len() {
        for second in first..=bytes.len() {
            assert_eq!(
                decode(&[&bytes[..first], &bytes[first..second], &bytes[second..]]),
                text,
                "split at {first}, {second}"
            );
        }
    }
    let chunks: Vec<_> = bytes.chunks(1).collect();
    assert_eq!(decode(&chunks), text);
}

#[test]
fn incomplete_code_points_wait_without_loss_or_duplication() {
    let mut decoder = StreamUtf8Decoder::default();
    let mut output = String::from("prefix:");
    for byte in &"🙂".as_bytes()[..3] {
        decoder.push(&[*byte], &mut output);
        decoder.push(&[], &mut output);
        assert_eq!(output, "prefix:");
    }
    decoder.push(&[0x82], &mut output);
    assert_eq!(output, "prefix:🙂");
    decoder.push("你好".as_bytes(), &mut output);
    decoder.finish(&mut output);
    decoder.finish(&mut output);
    assert_eq!(output, "prefix:🙂你好");
}

#[test]
fn incomplete_eof_uses_the_existing_lossy_replacement() {
    for character in ["é", "好", "🙂"] {
        for end in 1..character.len() {
            let bytes = &character.as_bytes()[..end];
            let chunks: Vec<_> = bytes.chunks(1).collect();
            assert_eq!(decode(&chunks), String::from_utf8_lossy(bytes));
            assert_eq!(decode(&chunks), "\u{fffd}");
        }
    }
}

#[test]
fn invalid_utf8_matches_whole_chunk_lossy_decoding() {
    let cases: &[&[u8]] = &[
        b"ok\xff\xfeend",
        b"\xc0\xaf\xe0\x80\xaf\xf0\x80\x80\xaf",
        b"\xed\xa0\x80\xf4\x90\x80\x80\xf5\x80\x80\x80",
        b"\xe2\x82X\xe2Y\xf0\x90\x80Z",
        b"\xe4\xbd\xa0\xff\xe5\xa5\xbd\xf0\x9f",
    ];
    for bytes in cases {
        let expected = String::from_utf8_lossy(bytes);
        assert_eq!(decode(&[bytes]), expected);
        for split in 0..=bytes.len() {
            assert_eq!(decode(&[&bytes[..split], &bytes[split..]]), expected);
        }
        assert_eq!(decode(&bytes.chunks(1).collect::<Vec<_>>()), expected);
    }
    for first in 0..=255 {
        for second in 0..=255 {
            let bytes = [first, second];
            assert_eq!(
                decode(&[&bytes[..1], &bytes[1..]]),
                String::from_utf8_lossy(&bytes)
            );
        }
    }
}

async fn provider_stream_preserves_text(provider_name: &str, body: &'static str) {
    let mut expected = None;
    for fragmented in [false, true] {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_header("content-type", "text/event-stream")
            .with_chunked_body(move |writer| {
                if fragmented {
                    // Flush each byte of non-ASCII code points separately over HTTP.
                    let mut start = 0;
                    for (index, byte) in body.bytes().enumerate() {
                        if !byte.is_ascii() {
                            writer.write_all(&body.as_bytes()[start..=index])?;
                            writer.flush()?;
                            std::thread::sleep(std::time::Duration::from_millis(10));
                            start = index + 1;
                        }
                    }
                    writer.write_all(&body.as_bytes()[start..])?;
                } else {
                    writer.write_all(body.as_bytes())?;
                }
                // An unterminated final line stays unparsed, including an incomplete code point.
                writer.write_all(b"data: unfinished \xe4\xb8")?;
                Ok(())
            })
            .create_async()
            .await;
        let provider: Box<dyn LlmProvider> = match provider_name {
            "openai" => Box::new(OpenAiFamilyProvider::new(
                "test-key".into(),
                server.url(),
                "test-model".into(),
                OpenAiFamilyAuthStyle::Bearer,
                vec![],
            )),
            "anthropic" => Box::new(AnthropicProvider::new(
                "test-key".into(),
                server.url(),
                "test-model".into(),
            )),
            "gemini" => Box::new(GeminiProvider::new(
                "test-key".into(),
                server.url(),
                "test-model".into(),
            )),
            "ollama" => Box::new(OllamaProvider::new(server.url(), "test-model".into())),
            _ => unreachable!(),
        };
        let chunks = Mutex::new(Vec::new());
        let request = LlmRequest::new(vec![ChatMessage::user("hello")]);
        let response = provider
            .complete_stream(&request, &|chunk| {
                chunks.lock().unwrap().push(serde_json::json!([
                    chunk.delta_text,
                    chunk.delta_reasoning,
                    chunk.delta_tool_call
                ]));
            })
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(response.message.text.as_deref(), Some("é你好🙂"));
        assert_eq!(response.message.usage.prompt_tokens, 7);
        assert_eq!(response.message.usage.completion_tokens, 3);
        assert_eq!(response.message.usage.total_tokens, 10);
        if matches!(provider_name, "openai" | "anthropic") {
            assert_eq!(response.message.reasoning_content.as_deref(), Some("想🙂"));
            assert!(matches!(response.message.stop_reason, StopReason::ToolUse));
        }
        if provider_name != "ollama" {
            assert_eq!(response.message.tool_calls.len(), 1);
            assert_eq!(response.message.tool_calls[0].tool_name, "lookup");
            assert_eq!(response.message.tool_calls[0].input["q"], "好🙂");
        } else {
            assert!(matches!(
                response.message.stop_reason,
                StopReason::MaxTokens
            ));
        }
        let actual = serde_json::json!([
            response.message,
            response.kv_cache_chunk_hashes,
            chunks.into_inner().unwrap()
        ]);
        if let Some(expected) = &expected {
            assert_eq!(&actual, expected);
        } else {
            expected = Some(actual);
        }
    }
}

#[tokio::test]
async fn openai_stream_preserves_fragmented_utf8() {
    provider_stream_preserves_text(
        "openai",
        concat!(
            "data: {\"id\":\"test\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"test-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"é你好🙂\",\"reasoning_content\":\"想🙂\"}}]}\r\n\r\n",
            "data: {\"id\":\"test\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"test-model\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\\\"好\"}}]}}]}\n\n",
            "data: {\"id\":\"test\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"test-model\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"🙂\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3,\"total_tokens\":10}}\n\n",
            "data: [DONE]\n\n"
        ),
    )
    .await;
}

#[tokio::test]
async fn anthropic_stream_preserves_fragmented_utf8() {
    provider_stream_preserves_text(
        "anthropic",
        concat!(
            "event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":7}}}\n\n",
            "event: content_block_delta\ndata: {\"delta\":{\"type\":\"text_delta\",\"text\":\"é你好🙂\"}}\n\n",
            "event: content_block_delta\ndata: {\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"想🙂\"}}\n\n",
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"lookup\"}}\n\n",
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"q\\\":\\\"好🙂\\\"}\"}}\n\n",
            "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\ndata: {}\n\n"
        ),
    )
    .await;
}

#[tokio::test]
async fn gemini_stream_preserves_fragmented_utf8() {
    provider_stream_preserves_text(
        "gemini",
        concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"é你好🙂\"}]}}]}\r\n\r\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"lookup\",\"args\":{\"q\":\"好🙂\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":7,\"candidatesTokenCount\":3}}\n\n"
        ),
    )
    .await;
}

#[tokio::test]
async fn ollama_stream_preserves_fragmented_utf8() {
    provider_stream_preserves_text(
        "ollama",
        concat!(
            "{\"message\":{\"content\":\"é你好🙂\"},\"done\":false}\r\n\n",
            "{\"message\":{\"content\":\"\"},\"done\":true,\"done_reason\":\"length\",\"prompt_eval_count\":7,\"eval_count\":3}\n"
        ),
    )
    .await;
}
