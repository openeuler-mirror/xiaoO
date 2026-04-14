use std::process;

use llm_client_cli::run;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("error: {}", err);
        process::exit(1);
    }
}
