mod cli;
mod config;
mod mcp;
mod memory_policy;
mod memory_ui;
mod theme;
mod tui;

fn main() -> std::process::ExitCode {
    cli::run()
}
