mod cli;
mod config;
mod mcp;
mod memory_policy;
mod tui;

fn main() -> std::process::ExitCode {
    cli::run()
}
