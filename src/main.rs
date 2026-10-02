mod cli;
mod config;
mod mcp;

fn main() -> std::process::ExitCode {
    cli::run()
}
