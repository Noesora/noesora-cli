mod cli;
mod cloud;
mod config;
mod mcp;
mod memory_policy;
mod memory_ui;
mod source_discovery;
mod theme;
mod tui;

fn main() -> std::process::ExitCode {
    cli::run()
}
