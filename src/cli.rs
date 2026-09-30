use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use noesora_engine::vault::{self, VaultError};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "noesora",
    version,
    about = "Local-first shared context for people and agents."
)]
struct Cli {
    /// Machine-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a vault in this directory.
    Init {
        /// Directory for the vault (default: current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show the nearest vault.
    Status,
}

#[derive(Serialize)]
struct OkInit {
    ok: bool,
    command: &'static str,
    vault: String,
}

#[derive(Serialize)]
struct OkStatus {
    ok: bool,
    command: &'static str,
    vault: String,
    version: u32,
    created_at: String,
}

#[derive(Serialize)]
struct Fail {
    ok: bool,
    error: String,
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match cli.command {
        None => {
            let _ = Cli::command().print_help();
            ExitCode::SUCCESS
        }
        Some(Command::Init { path }) => match vault::init_vault(&path) {
            Ok(root) => {
                emit_ok(
                    json,
                    OkInit {
                        ok: true,
                        command: "init",
                        vault: root.display().to_string(),
                    },
                    format!("Created vault at {}", root.display()),
                );
                ExitCode::SUCCESS
            }
            Err(err) => fail(json, err),
        },
        Some(Command::Status) => match status() {
            Ok((root, record)) => {
                emit_ok(
                    json,
                    OkStatus {
                        ok: true,
                        command: "status",
                        vault: root.display().to_string(),
                        version: record.version,
                        created_at: record.created_at.clone(),
                    },
                    format!(
                        "Vault {}\nversion {}  created {}",
                        root.display(),
                        record.version,
                        record.created_at
                    ),
                );
                ExitCode::SUCCESS
            }
            Err(err) => fail(json, err),
        },
    }
}

fn status() -> Result<(PathBuf, vault::VaultRecord), VaultError> {
    let cwd = std::env::current_dir()?;
    let root = vault::find_vault(&cwd).ok_or(VaultError::NotFound)?;
    let record = vault::read_vault(&root)?;
    Ok((root, record))
}

fn emit_ok<T: Serialize>(json: bool, payload: T, text: String) {
    if json {
        println!("{}", serde_json::to_string(&payload).expect("json"));
    } else {
        println!("{text}");
    }
}

fn fail(json: bool, err: VaultError) -> ExitCode {
    if json {
        let payload = Fail {
            ok: false,
            error: err.to_string(),
        };
        println!("{}", serde_json::to_string(&payload).expect("json"));
    } else {
        let _ = writeln!(io::stderr(), "noesora: {err}");
    }
    ExitCode::from(2)
}
