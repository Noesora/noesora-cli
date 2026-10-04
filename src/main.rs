mod cli;
mod config;
mod mcp;
mod tui;

fn main() -> std::process::ExitCode {
    cli::run()
}
