use std::io::{self, Write};

use agent_contracts::InteractionHandle;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;

pub struct CliInteractionHandle;

#[async_trait]
impl InteractionHandle for CliInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        match request {
            InteractionRequest::Confirm { prompt, .. } => {
                let input = prompt_input(&format!(
                    "[tool-cli][interaction.confirm] {} [y/N]: ",
                    prompt
                ));
                match input {
                    None => InteractionResponse::Unanswered,
                    Some(input) => InteractionResponse::Confirmed {
                        allowed: matches!(input.to_ascii_lowercase().as_str(), "y" | "yes"),
                    },
                }
            }
            InteractionRequest::TextInput {
                prompt, is_secret, ..
            } => {
                let input = prompt_input(&format!("[tool-cli][interaction.text] {}: ", prompt));
                // `display_value` is the downstream secret marker: the core loop
                // only treats a text answer as a secret when this field is
                // present, so a secret input must carry `<SECRET>` here just
                // like the other interaction backends.
                let display_value = if *is_secret {
                    Some("<SECRET>".to_string())
                } else {
                    None
                };
                InteractionResponse::Text {
                    value: input.filter(|value| !value.is_empty()),
                    display_value,
                }
            }
            InteractionRequest::Choice {
                prompt, options, ..
            } => {
                eprintln!("[tool-cli][interaction.choice] {}", prompt);
                for option in options {
                    eprintln!("  - {}", option);
                }
                let input = prompt_input("choice: ");
                InteractionResponse::Choice {
                    value: input.filter(|value| !value.is_empty()),
                }
            }
        }
    }
}

/// Read one trimmed line from stdin; `None` on EOF or read failure.
fn prompt_input(prompt: &str) -> Option<String> {
    print!("{}", prompt);
    let _ = io::stdout().flush();

    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(0) => None,
        Ok(_) => Some(input.trim().to_string()),
        Err(error) => {
            eprintln!(
                "[tool-cli][interaction.error] failed to read stdin: {}",
                error
            );
            None
        }
    }
}
