use agent_contracts::backend::{OperationBackendBuilder, OperationBackendConfig};
use operation_backend::OperationBackendBuilderImpl;
use serde_json::Value;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let kind = args
        .next()
        .ok_or_else(|| "usage: backend-cli <kind> [json-options]".to_string())?;
    let options = match args.next() {
        Some(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|error| format!("invalid backend options json: {error}"))?,
        None => Value::Null,
    };

    let config = OperationBackendConfig::new(kind, options);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|error| format!("failed to build tokio runtime: {error}"))?;
    let builder = OperationBackendBuilderImpl::new();

    match runtime.block_on(builder.build(&config)) {
        Ok(backend) => {
            println!("backend built: {}", backend.backend_id());
            Ok(())
        }
        Err(error) => Err(format!("backend build failed: {error}")),
    }
}
