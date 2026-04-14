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
                let allowed = matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes");
                InteractionResponse::Confirmed { allowed }
            }
            InteractionRequest::TextInput { prompt, .. } => {
                let input = prompt_input(&format!("[tool-cli][interaction.text] {}: ", prompt));
                InteractionResponse::Text {
                    value: if input.is_empty() { None } else { Some(input) },
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
                    value: if input.is_empty() { None } else { Some(input) },
                }
            }
        }
    }
}

fn prompt_input(prompt: &str) -> String {
    print!("{}", prompt);
    let _ = io::stdout().flush();

    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(_) => input.trim().to_string(),
        Err(error) => {
            eprintln!(
                "[tool-cli][interaction.error] failed to read stdin: {}",
                error
            );
            String::new()
        }
    }
}
