use clap::{Parser, Subcommand};
use moirai::{Span, SpanStorage, SqliteStorage};
use serde_json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[path = "../cli_api.rs"]
mod api;

#[derive(rust_embed::RustEmbed)]
#[folder = "static/"]
struct StaticAssets;

#[derive(Parser)]
#[command(name = "moirai")]
#[command(about = "CLI tool for viewing Moirai agent execution traces", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum Format {
    Json,
    Markdown,
}

#[derive(Subcommand)]
enum Commands {
    /// List traces from the database
    List {
        /// Path to the SQLite database
        #[arg(long, value_name = "PATH")]
        db: PathBuf,

        /// Number of traces to show (default: 10)
        #[arg(short, value_name = "COUNT", default_value = "10")]
        n: usize,

        /// Show only alive (ongoing) traces
        #[arg(long)]
        alive: bool,
    },

    /// View detailed log of a trace
    Log {
        /// Path to the SQLite database
        #[arg(long, value_name = "PATH")]
        db: PathBuf,

        /// Trace ID or prefix to look up
        #[arg(long, value_name = "ID")]
        trace_id: String,

        /// Show as ASCII tree graph
        #[arg(long)]
        graph: bool,
    },

    /// Export a trace to JSON or Markdown format
    Export {
        /// Path to the SQLite database
        #[arg(long, value_name = "PATH")]
        db: PathBuf,

        /// Trace ID or prefix to look up
        #[arg(long, value_name = "ID")]
        trace_id: String,

        /// Output format
        #[arg(long, value_name = "FORMAT", default_value = "json")]
        format: Format,
    },

    /// Serve traces via HTTP API
    Serve {
        /// Path to the SQLite database
        #[arg(long, value_name = "PATH")]
        db: Option<PathBuf>,

        /// Port to listen on
        #[arg(short, long, default_value = "9300")]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::List { db, n, alive } => {
            cmd_list(&db, n, alive).await?;
        }
        Commands::Log {
            db,
            trace_id,
            graph,
        } => {
            cmd_log(&db, &trace_id, graph).await?;
        }
        Commands::Export {
            db,
            trace_id,
            format,
        } => {
            cmd_export(&db, &trace_id, format).await?;
        }
        Commands::Serve { db, port } => {
            cmd_serve(&db, port).await?;
        }
    }

    Ok(())
}

async fn cmd_list(
    db_path: &PathBuf,
    limit: usize,
    alive: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteStorage::new(db_path.to_str().unwrap())?;

    let traces = if alive {
        storage.list_alive_traces(limit).await?
    } else {
        storage.list_traces(limit).await?
    };

    if traces.is_empty() {
        println!("No traces found");
        return Ok(());
    }

    let alive_trace_ids = if !alive {
        let alive_traces = storage.list_alive_traces(limit * 2).await?;
        alive_traces
            .iter()
            .map(|t| t.trace_id.clone())
            .collect::<std::collections::HashSet<_>>()
    } else {
        std::collections::HashSet::new()
    };

    println!(
        "{:<14} {:<6} {:<20} {:<10}",
        "TRACE_ID", "SPANS", "STARTED", "STATUS"
    );
    println!("{}", "-".repeat(50));

    for trace in traces {
        let trace_id_short = &trace.trace_id[..std::cmp::min(12, trace.trace_id.len())];
        let start_time = format_timestamp(trace.start_time);
        let status = if alive || alive_trace_ids.contains(&trace.trace_id) {
            "alive"
        } else {
            "ended"
        };

        println!(
            "{:<14} {:<6} {:<20} {:<10}",
            trace_id_short, trace.span_count, start_time, status
        );
    }

    Ok(())
}

async fn cmd_log(
    db_path: &PathBuf,
    trace_id_input: &str,
    graph: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteStorage::new(db_path.to_str().unwrap())?;

    let trace_id = storage
        .get_trace_by_prefix(trace_id_input)
        .await?
        .ok_or_else(|| format!("Trace not found: {}", trace_id_input))?;

    let spans = storage.get_trace_spans(&trace_id).await?;

    if spans.is_empty() {
        return Err(format!("No spans found for trace: {}", trace_id).into());
    }

    if graph {
        print_graph(&spans)?;
    } else {
        print_log(&spans)?;
    }

    Ok(())
}

fn format_timestamp(millis: i64) -> String {
    let secs = millis / 1000;
    let hour = (secs / 3600) % 24;
    let minute = (secs / 60) % 60;
    let second = secs % 60;
    format!("{:02}:{:02}:{:02}", hour, minute, second)
}

fn format_time_range(start: i64, end: Option<i64>) -> String {
    let start_str = format_timestamp(start);
    match end {
        Some(end_ms) => {
            let end_str = format_timestamp(end_ms);
            format!("{} - {}", start_str, end_str)
        }
        None => start_str,
    }
}

fn truncate_json(json: &serde_json::Value, max_len: usize) -> String {
    let s = json.to_string();
    if s.len() > max_len {
        format!("{}...", &s[..max_len - 3])
    } else {
        s
    }
}

fn print_log(spans: &[Span]) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{:<18} {:<10} {:<30} {:<40}",
        "SPAN_ID", "TYPE", "TIMING", "EXTRAS"
    );
    println!("{}", "-".repeat(98));

    for span in spans {
        let span_id_short = &span.span_id[..std::cmp::min(8, span.span_id.len())];
        let span_type = &span.span_type;
        let timing = format_time_range(span.start_time, span.end_time);
        let extras = truncate_json(&span.extras, 40);

        println!(
            "{:<18} {:<10} {:<30} {:<40}",
            span_id_short, span_type, timing, extras
        );
    }

    Ok(())
}

fn print_graph(spans: &[Span]) -> Result<(), Box<dyn std::error::Error>> {
    if spans.is_empty() {
        return Ok(());
    }

    let mut span_map: HashMap<String, &Span> = HashMap::new();
    for span in spans {
        span_map.insert(span.span_id.clone(), span);
    }

    let mut children_map: HashMap<String, Vec<&Span>> = HashMap::new();
    for span in spans {
        if let Some(parent_id) = &span.parent_span_id {
            children_map
                .entry(parent_id.clone())
                .or_insert_with(Vec::new)
                .push(span);
        }
    }

    for children in children_map.values_mut() {
        children.sort_by_key(|s| s.start_time);
    }

    let root_span = spans.iter().find(|s| s.parent_span_id.is_none());

    if let Some(root) = root_span {
        let timing = format_time_range(root.start_time, root.end_time);
        let root_id_short = &root.span_id[..std::cmp::min(8, root.span_id.len())];
        println!("{} ({}) {}", root.span_type, root_id_short, timing);

        if let Some(children) = children_map.get(&root.span_id) {
            print_tree_children(children, &children_map, "", true);
        }
    }

    Ok(())
}

fn print_tree_children(
    children: &[&Span],
    children_map: &HashMap<String, Vec<&Span>>,
    prefix: &str,
    _is_last_parent: bool,
) {
    for (i, span) in children.iter().enumerate() {
        let is_last = i == children.len() - 1;
        let branch = if is_last { "└── " } else { "├── " };

        let timing = format_time_range(span.start_time, span.end_time);
        let span_id_short = &span.span_id[..std::cmp::min(8, span.span_id.len())];
        println!(
            "{}{}{} ({}) {}",
            prefix, branch, span.span_type, span_id_short, timing
        );

        if let Some(grandchildren) = children_map.get(&span.span_id) {
            let continuation = if is_last { "    " } else { "│   " };
            let new_prefix = format!("{}{}", prefix, continuation);
            print_tree_children(grandchildren, children_map, &new_prefix, is_last);
        }
    }
}

async fn cmd_export(
    db_path: &PathBuf,
    trace_id_input: &str,
    format: Format,
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = SqliteStorage::new(db_path.to_str().unwrap())?;

    let trace_id = storage
        .get_trace_by_prefix(trace_id_input)
        .await?
        .ok_or_else(|| format!("Trace not found: {}", trace_id_input))?;

    let spans = storage.get_trace_spans(&trace_id).await?;

    if spans.is_empty() {
        return Err(format!("No spans found for trace: {}", trace_id).into());
    }

    match format {
        Format::Json => export_json(&spans),
        Format::Markdown => export_markdown(&spans),
    }
}

fn export_json(spans: &[Span]) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(spans)?;
    println!("{}", json);
    Ok(())
}

fn export_markdown(spans: &[Span]) -> Result<(), Box<dyn std::error::Error>> {
    let mut children_map: std::collections::HashMap<String, Vec<&Span>> =
        std::collections::HashMap::new();
    for span in spans {
        if let Some(parent_id) = &span.parent_span_id {
            children_map
                .entry(parent_id.clone())
                .or_insert_with(Vec::new)
                .push(span);
        }
    }

    for children in children_map.values_mut() {
        children.sort_by_key(|s| s.start_time);
    }

    let root_span = spans.iter().find(|s| s.parent_span_id.is_none());

    if let Some(root) = root_span {
        println!("# Trace: {}", root.trace_id);
        println!();
        println!("## Summary");
        println!();
        println!("- **Root Span ID**: {}", root.span_id);
        println!("- **Root Type**: {}", root.span_type);
        println!("- **Start Time**: {}", format_timestamp(root.start_time));
        if let Some(end_time) = root.end_time {
            println!("- **End Time**: {}", format_timestamp(end_time));
        }
        println!("- **Total Spans**: {}", spans.len());
        println!();
        println!("## Span Tree");
        println!();

        print_markdown_tree(root, &children_map, 0);
    } else {
        println!("# Trace (no root span found)");
        println!();
        println!("## Spans");
        println!();
        for span in spans {
            print_span_markdown(span, 0);
        }
    }

    Ok(())
}

fn print_markdown_tree(
    span: &Span,
    children_map: &std::collections::HashMap<String, Vec<&Span>>,
    depth: usize,
) {
    print_span_markdown(span, depth);

    if let Some(children) = children_map.get(&span.span_id) {
        for child in children {
            print_markdown_tree(child, children_map, depth + 1);
        }
    }
}

fn print_span_markdown(span: &Span, depth: usize) {
    let indent = "  ".repeat(depth);
    let timing = format_time_range(span.start_time, span.end_time);

    println!(
        "{}- **{}** (`{}`) - {}",
        indent,
        span.span_type,
        &span.span_id[..std::cmp::min(8, span.span_id.len())],
        timing
    );

    if !span.extras.is_null() {
        let extras_str = truncate_json(&span.extras, 80);
        println!("{}  *Extras*: {}", indent, extras_str);
    }
}

async fn cmd_serve(db: &Option<PathBuf>, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    use axum::{routing::get, Router};
    use std::net::SocketAddr;

    let db_path = discover_db_path(db)?;

    if !db_path.exists() {
        return Err(format!(
            "Database not found at: {}\n\nCreate a database and add spans programmatically.",
            db_path.display()
        )
        .into());
    }

    let storage = Arc::new(SqliteStorage::new(db_path.to_str().unwrap())?);
    let state = api::AppState { storage };

    let app = Router::new()
        .route("/api/traces", get(api::list_traces))
        .route(
            "/api/traces/:id",
            get(api::get_trace).delete(api::delete_trace),
        )
        .route("/api/spans/:id", get(api::get_span))
        .fallback(static_handler)
        .with_state(state);

    let mut current_port = port;
    let max_attempts = 10;

    for attempt in 0..max_attempts {
        let addr = SocketAddr::from(([0, 0, 0, 0], current_port));

        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                println!("Server running on http://localhost:{}", current_port);
                println!("Database: {}", db_path.display());
                axum::serve(listener, app).await?;
                return Ok(());
            }
            Err(_) if attempt < max_attempts - 1 => {
                current_port += 1;
            }
            Err(e) => {
                return Err(format!(
                    "Failed to bind to any port in range {}-{}: {}",
                    port,
                    port + max_attempts - 1,
                    e
                )
                .into());
            }
        }
    }

    Ok(())
}

fn discover_db_path(db: &Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = db {
        return Ok(path.clone());
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let default_path = PathBuf::from(&home).join(".moirai").join("traces.db");

    if default_path.exists() {
        return Ok(default_path);
    }

    let mut current_dir = std::env::current_dir()?;
    loop {
        let candidate = current_dir.join(".moirai").join("traces.db");
        if candidate.exists() {
            return Ok(candidate);
        }

        match current_dir.parent() {
            Some(parent) => current_dir = parent.to_path_buf(),
            None => break,
        }
    }

    Ok(default_path)
}

async fn static_handler(uri: axum::http::Uri) -> impl axum::response::IntoResponse {
    use axum::{
        body::Body,
        http::{header, StatusCode},
        response::Response,
    };

    let path = uri.path().trim_start_matches('/');

    let path = if path.is_empty() || path == "index.html" {
        "index.html"
    } else {
        path
    };

    match StaticAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(content.data))
                .unwrap()
        }
        None => match StaticAssets::get("index.html") {
            Some(content) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html")
                .body(Body::from(content.data))
                .unwrap(),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("404 Not Found"))
                .unwrap(),
        },
    }
}
