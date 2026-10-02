use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use noesora_engine::index::{self, IndexError, SearchResult};
use noesora_engine::record::{self, RecordError, RecordStatus};
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
    /// Write a durable note into the vault.
    Note {
        /// One-line title (required).
        #[arg(long)]
        title: String,
        /// open, done, or blocked.
        #[arg(long, default_value = "open")]
        status: String,
        /// Paths this note is about.
        #[arg(long = "file")]
        files: Vec<String>,
        /// Note body.
        body: Vec<String>,
    },
    /// Search local records with citations or refuse when evidence is missing.
    Search { query: String },
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
struct OkNote {
    ok: bool,
    command: &'static str,
    id: String,
    path: String,
    vault: String,
}

#[derive(Serialize)]
struct OkSearch {
    ok: bool,
    command: &'static str,
    hits: Vec<index::SearchHit>,
    refused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

#[derive(Serialize)]
struct Fail {
    ok: bool,
    error: String,
}

pub fn run() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return clap_error(err),
    };
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
        Some(Command::Note {
            title,
            status,
            files,
            body,
        }) => match note(title, status, files, body) {
            Ok(written) => {
                emit_ok(
                    json,
                    OkNote {
                        ok: true,
                        command: "note",
                        id: written.id.clone(),
                        path: written.path.display().to_string(),
                        vault: written.vault.display().to_string(),
                    },
                    format!("Wrote note {} at {}", written.id, written.path.display()),
                );
                ExitCode::SUCCESS
            }
            Err(err) => fail(json, err),
        },
        Some(Command::Search { query }) => match search(&query) {
            Ok(result) => {
                let (hits, reason) = match result {
                    SearchResult::Hits(hits) => (hits, None),
                    SearchResult::Refused => (Vec::new(), Some("no_evidence")),
                };
                if json {
                    let payload = OkSearch {
                        ok: true,
                        command: "search",
                        hits,
                        refused: reason.is_some(),
                        reason,
                    };
                    println!("{}", serde_json::to_string(&payload).expect("json"));
                } else if reason.is_some() {
                    println!("Refused: no evidence found.");
                } else {
                    for hit in hits {
                        println!(
                            "{} [{}] {}:{} {}\n{}",
                            hit.title, hit.kind, hit.path, hit.span, hit.hash, hit.text
                        );
                    }
                }
                ExitCode::SUCCESS
            }
            Err(err) => fail(json, err),
        },
    }
}

fn search(query: &str) -> Result<SearchResult, IndexError> {
    let cwd = std::env::current_dir()?;
    let root = vault::find_vault(&cwd).ok_or(VaultError::NotFound)?;
    index::search(&root, query)
}

fn status() -> Result<(PathBuf, vault::VaultRecord), VaultError> {
    let cwd = std::env::current_dir()?;
    let root = vault::find_vault(&cwd).ok_or(VaultError::NotFound)?;
    let record = vault::read_vault(&root)?;
    Ok((root, record))
}

fn note(
    title: String,
    status: String,
    files: Vec<String>,
    body: Vec<String>,
) -> Result<record::WrittenNote, RecordError> {
    let status = RecordStatus::parse(&status).ok_or(RecordError::InvalidStatus)?;
    let cwd = std::env::current_dir().map_err(RecordError::from)?;
    record::write_note(&cwd, &title, &body.join(" "), status, &files)
}
fn emit_ok<T: Serialize>(json: bool, payload: T, text: String) {
    if json {
        println!("{}", serde_json::to_string(&payload).expect("json"));
    } else {
        println!("{text}");
    }
}

fn clap_error(err: clap::Error) -> ExitCode {
    let json = std::env::args_os().any(|arg| arg == "--json");
    if json && err.exit_code() != 0 {
        return fail(true, err);
    }
    let code = err.exit_code();
    let _ = err.print();
    ExitCode::from(code as u8)
}

fn fail(json: bool, err: impl std::fmt::Display) -> ExitCode {
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
