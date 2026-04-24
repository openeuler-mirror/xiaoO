#[path = "backend_cli/app.rs"]
mod app;

fn main() {
    if let Err(error) = app::run_from_env() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
