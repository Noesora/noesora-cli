use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::config::{self, ConfigError};
use crate::mcp;
use clap::{CommandFactory, Parser, Subcommand};
use noesora_engine::candidate;
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
    /// Run one read-only SQL query against the local index.
    Query { sql: String },
    /// Serve search, query, note and handoff over stdio MCP for the global default vault.
    Mcp,
    /// Import a caller-selected transcript into the nearest vault as an unindexed candidate.
    Candidate {
        #[command(subcommand)]
        action: CandidateCommand,
    },
    /// Manage the user-global default vault used by MCP.
    Vault {
        #[command(subcommand)]
        action: VaultCommand,
    },
}

#[derive(Subcommand)]
enum VaultCommand {
    /// Validate a vault and make it the global default for MCP.
    Use {
        /// Vault root (the directory holding `.noesora/vault.json`).
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum CandidateCommand {
    /// Import a transcript file the caller has confirmed is complete.
    Import { path: PathBuf },
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
pub(crate) struct OkSearch {
    pub(crate) ok: bool,
    pub(crate) command: &'static str,
    pub(crate) hits: Vec<index::SearchHit>,
    pub(crate) refused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<&'static str>,
}

impl OkSearch {
    pub(crate) fn from_result(command: &'static str, result: SearchResult) -> Self {
        let (hits, reason) = match result {
            SearchResult::Hits(hits) => (hits, None),
            SearchResult::Refused => (Vec::new(), Some("no_evidence")),
        };
        OkSearch {
            ok: true,
            command,
            hits,
            refused: reason.is_some(),
            reason,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct OkQuery {
    pub(crate) ok: bool,
    pub(crate) command: &'static str,
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<String>>,
    pub(crate) truncated: bool,
}

impl OkQuery {
    pub(crate) fn from_result(result: index::QueryResult) -> Self {
        let index::QueryResult {
            columns,
            rows,
            truncated,
        } = result;
        OkQuery {
            ok: true,
            command: "query",
            columns,
            rows,
            truncated,
        }
    }
}

#[derive(Serialize)]
struct OkVaultUse {
    ok: bool,
    command: &'static str,
    vault: String,
    config: String,
}

#[derive(Serialize)]
struct OkCandidateImport {
    ok: bool,
    command: &'static str,
    path: String,
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
                let payload = OkSearch::from_result("search", result);
                if json {
                    println!("{}", serde_json::to_string(&payload).expect("json"));
                } else if payload.refused {
                    println!("Refused: no evidence found.");
                } else {
                    for hit in payload.hits {
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
        Some(Command::Mcp) => mcp::run(),
        Some(Command::Query { sql }) => match query(&sql) {
            Ok(result) => {
                if json {
                    let payload = OkQuery::from_result(result);
                    println!("{}", serde_json::to_string(&payload).expect("json"));
                } else {
                    print!("{}", result.csv());
                    if result.truncated {
                        let _ = writeln!(
                            io::stderr(),
                            "noesora: result capped at {} rows",
                            index::QUERY_ROW_CAP
                        );
                    }
                }
                ExitCode::SUCCESS
            }
            Err(err) => fail(json, err),
        },
        Some(Command::Candidate {
            action: CandidateCommand::Import { path },
        }) => match candidate_import(&path) {
            Ok(path) => {
                emit_ok(
                    json,
                    OkCandidateImport {
                        ok: true,
                        command: "candidate import",
                        path: path.display().to_string(),
                    },
                    format!("Imported transcript candidate to {}", path.display()),
                );
                ExitCode::SUCCESS
            }
            Err(err) => fail(json, err),
        },
        Some(Command::Vault {
            action: VaultCommand::Use { path },
        }) => match vault_use(&path) {
            Ok((root, file)) => {
                emit_ok(
                    json,
                    OkVaultUse {
                        ok: true,
                        command: "vault use",
                        vault: root.display().to_string(),
                        config: file.display().to_string(),
                    },
                    format!("Default vault set to {}", root.display()),
                );
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

fn query(sql: &str) -> Result<index::QueryResult, IndexError> {
    let cwd = std::env::current_dir()?;
    let root = vault::find_vault(&cwd).ok_or(VaultError::NotFound)?;
    index::query(&root, sql)
}

fn status() -> Result<(PathBuf, vault::VaultRecord), VaultError> {
    let cwd = std::env::current_dir()?;
    let root = vault::find_vault(&cwd).ok_or(VaultError::NotFound)?;
    let record = vault::read_vault(&root)?;
    Ok((root, record))
}

fn vault_use(path: &std::path::Path) -> Result<(PathBuf, PathBuf), ConfigError> {
    config::set_default_vault(&config::home_dir()?, path)
}
fn candidate_import(source: &std::path::Path) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir()
        .map_err(|err| format!("could not determine current directory: {err}"))?;
    let transcript = File::open(source)
        .map_err(|err| format!("could not open transcript {}: {err}", source.display()))?;
    candidate::write_transcript_candidate(&cwd, transcript).map_err(|err| err.to_string())
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
    // MCP mode keeps stdout for protocol frames only, so `--json` never applies to it.
    let mcp = std::env::args_os()
        .skip(1)
        .find(|arg| !arg.to_string_lossy().starts_with('-'))
        .is_some_and(|arg| arg == "mcp");
    let json = !mcp && std::env::args_os().any(|arg| arg == "--json");
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
