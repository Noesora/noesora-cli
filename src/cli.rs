use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use noesora_engine::{Client, ContentScope, RecordInput, SearchFilters, SearchMode, Store};
use serde::Serialize;
use serde_json::{json, Value};

use crate::{config, mcp, memory_policy::MemoryPolicy};

#[derive(Parser)]
#[command(
    name = "noesora",
    version,
    about = "Local-first session workspace and cited search."
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
    /// Initialize the user-global store.
    Init,
    /// Show status for the user-global store.
    Status,
    /// Manage imported raw session history.
    Session {
        #[command(subcommand)]
        action: SessionCommand,
    },
    /// Manage explicitly enabled source roots.
    Source {
        #[command(subcommand)]
        action: SourceCommand,
    },
    /// Import only configured, enabled source roots.
    Sync,
    /// Inspect or explicitly install the local embedding model.
    Model {
        #[command(subcommand)]
        action: ModelCommand,
    },
    /// Build pending local semantic embeddings.
    Index,
    /// Rebuild derived store data from canonical snapshots and records.
    Rebuild,
    /// Import existing Markdown records without modifying their originals.
    ImportVault { path: PathBuf },
    /// Search local evidence with source citations.
    Search {
        /// Text or semantic retrieval over the local store.
        query: String,
        #[arg(long, value_enum, default_value = "text")]
        mode: SearchModeArg,
        /// Restrict to one project; all projects are searched by default.
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_enum)]
        client: Option<SearchClientArg>,
        /// Restrict results to one exact source namespace; independent of native client.
        #[arg(long = "source-namespace")]
        source_namespace: Option<String>,
        #[arg(long, value_enum, default_value = "conversation")]
        scope: ContentScopeArg,
        /// Maximum returned passages, between 1 and 100.
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
    },
    /// Write a durable approved note.
    Note {
        /// One-line title.
        #[arg(long)]
        title: String,
        /// open, done, or blocked.
        #[arg(long, default_value = "open")]
        status: String,
        /// Paths this note is about.
        #[arg(long = "file")]
        files: Vec<String>,
        /// Optional project metadata.
        #[arg(long)]
        project: Option<String>,
        /// Note body.
        body: Vec<String>,
    },
    /// Run one read-only, row-capped SQL query and print CSV.
    Query { sql: String },
    /// Serve shared engine tools over MCP stdio.
    Mcp {
        /// Restrict this MCP connection to an explicit read-only memory scope.
        #[arg(long)]
        managed: bool,
        /// Exact persisted project identity exposed by this managed connection.
        #[arg(long, requires = "managed", conflicts_with = "all_projects")]
        project: Option<String>,
        /// Explicitly expose read-only memory across all projects.
        #[arg(
            long = "all-projects",
            requires = "managed",
            conflicts_with = "project"
        )]
        all_projects: bool,
    },
    /// Browse local memory with optional search-scope filters.
    Tui {
        /// Limit search to one exact project; session browsing remains global.
        #[arg(long)]
        project: Option<String>,
        /// Limit search to one native client.
        #[arg(long, value_enum)]
        client: Option<SearchClientArg>,
        /// Limit search to one exact source namespace; independent of native client.
        #[arg(long = "source-namespace")]
        source_namespace: Option<String>,
    },
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Explicitly archive one session JSONL file.
    Import {
        #[arg(long, value_enum)]
        client: IntakeClientArg,
        path: PathBuf,
    },
    /// Ingest one normalized noesora.session/v1 JSONL file.
    Ingest { path: PathBuf },
    /// List global sessions, optionally filtered by project.
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Read one paged raw transcript.
    Show {
        id: String,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long, default_value = "40")]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum SourceCommand {
    /// Add an explicitly enabled session source root.
    Add {
        #[arg(long, value_enum)]
        client: IntakeClientArg,
        path: PathBuf,
    },
    /// List enabled and disabled source roots.
    List,
}

#[derive(Subcommand)]
enum ModelCommand {
    /// Explicitly install the pinned local embedding model assets.
    Install,
    /// Inspect local model availability without network access.
    Status,
}

#[derive(Clone, Copy, ValueEnum)]
enum IntakeClientArg {
    Omp,
    #[value(name = "claude-code")]
    ClaudeCode,
    Codex,
}

impl From<IntakeClientArg> for Client {
    fn from(value: IntakeClientArg) -> Self {
        match value {
            IntakeClientArg::Omp => Self::Omp,
            IntakeClientArg::ClaudeCode => Self::ClaudeCode,
            IntakeClientArg::Codex => Self::Codex,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SearchClientArg {
    Omp,
    #[value(name = "claude-code")]
    ClaudeCode,
    Codex,
    Grok,
    #[value(name = "opencode")]
    OpenCode,
    #[value(name = "cursor-agent")]
    Cursor,
}

impl From<SearchClientArg> for Client {
    fn from(value: SearchClientArg) -> Self {
        match value {
            SearchClientArg::Omp => Self::Omp,
            SearchClientArg::ClaudeCode => Self::ClaudeCode,
            SearchClientArg::Codex => Self::Codex,
            SearchClientArg::Grok => Self::Grok,
            SearchClientArg::OpenCode => Self::OpenCode,
            SearchClientArg::Cursor => Self::Cursor,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SearchModeArg {
    Text,
    Semantic,
}

impl From<SearchModeArg> for SearchMode {
    fn from(value: SearchModeArg) -> Self {
        match value {
            SearchModeArg::Text => Self::Text,
            SearchModeArg::Semantic => Self::Semantic,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ContentScopeArg {
    Conversation,
    Tools,
    All,
}

impl From<ContentScopeArg> for ContentScope {
    fn from(value: ContentScopeArg) -> Self {
        match value {
            ContentScopeArg::Conversation => Self::Conversation,
            ContentScopeArg::Tools => Self::Tools,
            ContentScopeArg::All => Self::All,
        }
    }
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
    let Some(command) = cli.command else {
        let _ = Cli::command().print_help();
        return ExitCode::SUCCESS;
    };

    match command {
        Command::Mcp {
            managed,
            project,
            all_projects,
        } => match mcp_memory_policy(managed, project, all_projects) {
            Ok(policy) => mcp::run(policy),
            Err(err) => fail(json, err),
        },
        Command::Tui {
            project,
            client,
            source_namespace,
        } => {
            if json {
                return fail(true, "the tui command does not support --json");
            }
            let filters = SearchFilters {
                project,
                client: client.map(Into::into),
                source_namespace,
                scope: ContentScope::Conversation,
                limit: 10,
            };
            match config::open_store().and_then(|store| crate::tui::run(store, filters)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => fail(false, err),
            }
        }
        command => match config::open_store() {
            Ok(store) => {
                if json {
                    if let Err(err) = config::require_json_safe_store(&store) {
                        return fail(true, err);
                    }
                }
                run_store_command(json, store, command)
            }
            Err(err) => fail(json, err),
        },
    }
}

fn run_store_command(json_output: bool, store: Store, command: Command) -> ExitCode {
    match command {
        Command::Init => match store.status() {
            Ok(status) => {
                let text = format!("Initialized Noesora store at {}", status.root.display());
                let payload = if json_output {
                    let root = match serde_json::to_value(&status.root) {
                        Ok(root) => root,
                        Err(err) => return fail(true, err),
                    };
                    json!({"ok": true, "command": "init", "root": root})
                } else {
                    Value::Null
                };
                emit(json_output, payload, text);
                ExitCode::SUCCESS
            }
            Err(err) => fail(json_output, err),
        },
        Command::Status => match store.status() {
            Ok(status) => {
                let text = format_store_status(&status);
                emit_command(json_output, "status", status, text)
            }
            Err(err) => fail(json_output, err),
        },
        Command::Session { action } => match action {
            SessionCommand::Import { client, path } => {
                match store.import_session(client.into(), &path) {
                    Ok(report) => {
                        let text = format_import(&report);
                        emit_command(json_output, "session import", report, text)
                    }
                    Err(err) => fail(json_output, err),
                }
            }
            SessionCommand::Ingest { path } => match store.ingest_session(&path) {
                Ok(report) => {
                    let text = format_ingest(&report);
                    emit_command(json_output, "session ingest", report, text)
                }
                Err(err) => fail(json_output, err),
            },
            SessionCommand::List {
                project,
                offset,
                limit,
            } => match store.sessions(project.as_deref(), offset, limit) {
                Ok(sessions) => {
                    let text = format_sessions(&sessions, offset);
                    emit(
                        json_output,
                        json!({"ok": true, "sessions": sessions, "offset": offset}),
                        text,
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => fail(json_output, err),
            },
            SessionCommand::Show { id, offset, limit } => {
                match store.read_session(&id, offset, limit) {
                    Ok(page) => {
                        let text = format_transcript(&page);
                        emit_command(json_output, "session show", page, text)
                    }
                    Err(err) => fail(json_output, err),
                }
            }
        },
        Command::Source { action } => match action {
            SourceCommand::Add { client, path } => match store.add_source(client.into(), &path) {
                Ok(source) => {
                    let text = format!(
                        "{} {} source at {}",
                        if source.enabled {
                            "Enabled"
                        } else {
                            "Disabled"
                        },
                        source.client,
                        source.path.display()
                    );
                    emit_command(json_output, "source add", source, text)
                }
                Err(err) => fail(json_output, err),
            },
            SourceCommand::List => match store.sources() {
                Ok(sources) => {
                    let text = format_sources(&sources);
                    let payload = if json_output {
                        let sources = match serde_json::to_value(&sources) {
                            Ok(sources) => sources,
                            Err(err) => return fail(true, err),
                        };
                        json!({"ok": true, "sources": sources})
                    } else {
                        Value::Null
                    };
                    emit(json_output, payload, text);
                    ExitCode::SUCCESS
                }
                Err(err) => fail(json_output, err),
            },
        },
        Command::Sync => match store.sync() {
            Ok(report) => {
                let text = format_sync(&report);
                let has_errors = !report.errors.is_empty();
                let payload = if json_output {
                    let imports = match serde_json::to_value(&report.imports) {
                        Ok(imports) => imports,
                        Err(err) => return fail(true, err),
                    };
                    let errors = match serde_json::to_value(&report.errors) {
                        Ok(errors) => errors,
                        Err(err) => return fail(true, err),
                    };
                    if has_errors {
                        let error = format!(
                            "{} configured source(s) failed to sync",
                            report.errors.len()
                        );
                        json!({
                            "ok": false,
                            "error": error,
                            "imports": imports,
                            "errors": errors,
                        })
                    } else {
                        json!({"ok": true, "imports": imports, "errors": errors})
                    }
                } else {
                    Value::Null
                };
                emit(json_output, payload, text);
                if has_errors {
                    ExitCode::from(2)
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(err) if json_output => {
                let error = err.to_string();
                emit(
                    true,
                    json!({"ok": false, "error": error, "imports": [], "errors": []}),
                    String::new(),
                );
                ExitCode::from(2)
            }
            Err(err) => fail(false, err),
        },
        Command::Model { action } => match action {
            ModelCommand::Install => {
                match noesora_engine::embedding::install_model(&store.model_dir()) {
                    Ok(model) => {
                        let text = format_model(&model);
                        emit_command(json_output, "model install", model, text)
                    }
                    Err(err) => fail(json_output, err),
                }
            }
            ModelCommand::Status => {
                let model = noesora_engine::embedding::model_status(&store.model_dir());
                let text = format_model(&model);
                emit_command(json_output, "model status", model, text)
            }
        },
        Command::Index => match store.index_embeddings() {
            Ok(report) => {
                let text = format!(
                    "Indexed {} passages; {} remain (model fingerprint {}).",
                    report.indexed, report.remaining, report.fingerprint
                );
                emit_command(json_output, "index", report, text)
            }
            Err(err) => fail(json_output, err),
        },
        Command::Rebuild => match store.rebuild() {
            Ok(status) => {
                let text = format_store_status(&status);
                emit_command(json_output, "rebuild", status, text)
            }
            Err(err) => fail(json_output, err),
        },
        Command::ImportVault { path } => match store.import_vault(&path) {
            Ok(imported) => {
                emit(
                    json_output,
                    json!({"ok": true, "command": "import-vault", "imported": imported}),
                    format!("Imported {imported} records from {}", path.display()),
                );
                ExitCode::SUCCESS
            }
            Err(err) => fail(json_output, err),
        },
        Command::Search {
            query,
            mode,
            project,
            client,
            source_namespace,
            scope,
            limit,
        } => {
            let filters = SearchFilters {
                project,
                client: client.map(Into::into),
                source_namespace,
                scope: scope.into(),
                limit: usize::from(limit),
            };
            match store.search(&query, mode.into(), &filters) {
                Ok(response) => {
                    let text = format_search(&response);
                    let truncated = response.truncated;
                    let exit = emit_command(json_output, "search", response, text);
                    if truncated && !json_output {
                        let _ = writeln!(
                            io::stderr(),
                            "noesora: more ranked hits exist; showing the returned evidence"
                        );
                    }
                    exit
                }
                Err(err) => fail_search(json_output, err),
            }
        }
        Command::Note {
            title,
            status,
            files,
            project,
            body,
        } => {
            let input = RecordInput {
                title,
                body: body.join(" "),
                kind: "note".to_owned(),
                status,
                project: project.unwrap_or_else(|| "local".to_owned()),
                files,
            };
            match store.write_record(&input) {
                Ok(written) => {
                    let text = format!("Wrote note {} at {}", written.id, written.path.display());
                    emit_command(json_output, "note", written, text)
                }
                Err(err) => fail(json_output, err),
            }
        }
        Command::Query { sql } => match store.query(&sql) {
            Ok(result) => {
                let truncated = result.truncated;
                let text = result.csv();
                let exit = emit_command(json_output, "query", result, text);
                if truncated && !json_output {
                    let _ = writeln!(
                        io::stderr(),
                        "noesora: query result was capped by the engine"
                    );
                }
                exit
            }
            Err(err) => fail(json_output, err),
        },
        Command::Mcp { .. } | Command::Tui { .. } => {
            unreachable!("handled before opening the store")
        }
    }
}
fn mcp_memory_policy(
    managed: bool,
    project: Option<String>,
    all_projects: bool,
) -> Result<MemoryPolicy, String> {
    if !managed {
        if project.is_some() || all_projects {
            return Err("MCP scope flags require --managed".to_owned());
        }
        return Ok(MemoryPolicy::Owner);
    }

    match (project, all_projects) {
        (Some(project), false) => MemoryPolicy::project(project),
        (None, true) => Ok(MemoryPolicy::AllReadOnly),
        (None, false) => Err(
            "--managed requires exactly one of --project <exact-id> or --all-projects".to_owned(),
        ),
        (Some(_), true) => {
            Err("--project and --all-projects are conflicting managed scopes".to_owned())
        }
    }
}

fn with_command<T: Serialize>(
    command: &'static str,
    payload: T,
) -> Result<Value, serde_json::Error> {
    let payload = serde_json::to_value(payload)?;
    Ok(match payload {
        Value::Object(mut object) => {
            object.insert("ok".to_owned(), Value::Bool(true));
            object.insert("command".to_owned(), Value::String(command.to_owned()));
            Value::Object(object)
        }
        value => json!({"ok": true, "command": command, "payload": value}),
    })
}

fn emit_command<T: Serialize>(
    json_output: bool,
    command: &'static str,
    payload: T,
    text: String,
) -> ExitCode {
    if !json_output {
        println!("{}", clean_terminal(&text));
        return ExitCode::SUCCESS;
    }
    match with_command(command, payload) {
        Ok(payload) => {
            println!("{payload}");
            ExitCode::SUCCESS
        }
        Err(err) => fail(true, err),
    }
}

fn format_store_status(status: &noesora_engine::StoreStatus) -> String {
    format!(
        "Store {}
{} sessions · {} messages · {} records
{} text chunks · {} embedded · {} pending embeddings",
        clean_terminal(&status.root.display().to_string()),
        status.sessions,
        status.messages,
        status.records,
        status.chunks,
        status.embedded_chunks,
        status.pending_embeddings,
    )
}

fn format_import(report: &noesora_engine::ImportReport) -> String {
    format!(
        "{} session {} · {} · {} messages · {} chunks{}
Snapshot: {}",
        report.client,
        clean_terminal(&report.session_id),
        clean_terminal(&report.project),
        report.messages,
        report.chunks,
        if report.duplicate {
            " (duplicate; already imported)"
        } else {
            ""
        },
        clean_terminal(&report.snapshot.display().to_string()),
    )
}
fn format_ingest(report: &noesora_engine::SessionIngestReport) -> String {
    format!(
        "Ingested session {} · source namespace {} · external ID {} · project {}\n{} messages · {} chunks{}\nSnapshot: {}",
        clean_terminal(&report.session_id),
        clean_terminal(&report.source_namespace),
        clean_terminal(&report.external_id),
        clean_terminal(&report.project),
        report.messages,
        report.chunks,
        if report.duplicate {
            " (duplicate; already ingested)"
        } else {
            ""
        },
        clean_terminal(&report.snapshot.display().to_string()),
    )
}

fn native_client_label(client: Option<&Client>) -> &'static str {
    match client {
        Some(Client::Omp) => "omp",
        Some(Client::ClaudeCode) => "claude-code",
        Some(Client::Codex) => "codex",
        Some(Client::Grok) => "grok",
        Some(Client::OpenCode) => "opencode",
        Some(Client::Cursor) => "cursor-agent",
        None => "none",
    }
}

fn source_time(timestamp: Option<&str>) -> &str {
    timestamp.unwrap_or("unknown source time")
}

fn format_sessions(sessions: &[noesora_engine::SessionSummary], offset: usize) -> String {
    if sessions.is_empty() {
        return "No imported sessions on this page.".to_owned();
    }
    use std::fmt::Write as _;
    let mut output = String::new();
    for (index, session) in sessions.iter().enumerate() {
        if index != 0 {
            output.push('\n');
        }
        let _ = write!(
            output,
            "{}. {} [project {} · source namespace {} · external ID {} · native client {}] {} · {} messages · created {} · updated {}",
            offset + index + 1,
            clean_terminal(&session.id),
            clean_terminal(&session.project),
            clean_terminal(&session.source_namespace),
            clean_terminal(&session.external_id),
            native_client_label(session.client.as_ref()),
            clean_terminal(&session.title),
            session.messages,
            clean_terminal(source_time(session.created_at.as_deref())),
            clean_terminal(source_time(session.updated_at.as_deref())),
        );
    }
    output
}

fn format_transcript(page: &noesora_engine::TranscriptPage) -> String {
    let mut output = format!(
        "Raw session {} · project {} · source namespace {} · external ID {}\nNative client: {}\nSource created: {} · updated: {}\nSnapshot: {}\nMessages {}–{} of {}\n",
        clean_terminal(&page.session.id),
        clean_terminal(&page.session.project),
        clean_terminal(&page.session.source_namespace),
        clean_terminal(&page.session.external_id),
        native_client_label(page.session.client.as_ref()),
        clean_terminal(source_time(page.session.created_at.as_deref())),
        clean_terminal(source_time(page.session.updated_at.as_deref())),
        clean_terminal(&page.session.snapshot),
        page.offset.saturating_add(1),
        page.offset.saturating_add(page.messages.len()),
        page.total,
    );
    for message in &page.messages {
        output.push_str(&format!(
            "\n[{} · {} · {}]\n{}\n{}\n",
            clean_terminal(&message.role),
            clean_terminal(&message.kind),
            clean_terminal(message.timestamp.as_deref().unwrap_or("-")),
            clean_terminal(&message.id),
            clean_terminal(&message.text),
        ));
    }
    output
}

fn format_sources(sources: &[noesora_engine::SourceConfig]) -> String {
    if sources.is_empty() {
        return "No source roots configured.".to_owned();
    }
    sources
        .iter()
        .map(|source| {
            format!(
                "{} [{}] {} · {}",
                clean_terminal(&source.id),
                source.client,
                clean_terminal(&source.path.display().to_string()),
                if source.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
            )
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn format_sync(report: &noesora_engine::SyncReport) -> String {
    let mut output = format!(
        "Imported {} sessions from configured sources.",
        report.imports.len()
    );
    for error in &report.errors {
        output.push_str(
            "
Source error: ",
        );
        output.push_str(&clean_terminal(error));
    }
    output
}

fn format_model(model: &noesora_engine::ModelStatus) -> String {
    format!(
        "Model {} · {} · {} · {}",
        if model.installed {
            "installed"
        } else {
            "unavailable"
        },
        model.model_id,
        model.revision,
        clean_terminal(&model.path.display().to_string()),
    )
}

fn format_search(response: &noesora_engine::SearchResponse) -> String {
    if response.refused {
        return format!(
            "Refused: no evidence found ({})",
            response.reason.as_deref().unwrap_or("no_evidence")
        );
    }
    if response.hits.is_empty() {
        return "No evidence found.".to_owned();
    }
    response
        .hits
        .iter()
        .map(|hit| {
            let origin = if hit.session_id.is_some() {
                format!(
                    "Session: {}\nMessage: {}\nSource namespace: {}\nExternal ID: {}\nNative client: {}",
                    clean_terminal(hit.session_id.as_deref().unwrap_or("unknown session")),
                    clean_terminal(hit.message_id.as_deref().unwrap_or("unknown message")),
                    clean_terminal(
                        hit.source_namespace
                            .as_deref()
                            .unwrap_or("unknown source namespace")
                    ),
                    clean_terminal(hit.external_id.as_deref().unwrap_or("unknown external ID")),
                    native_client_label(hit.client.as_ref()),
                )
            } else {
                "Source: approved record".to_owned()
            };
            format!(
                "{} [{}] {}\nID: {}\nType: {}\nRole: {}\n{}\nPath: {}\nSpan: {}..{}\nHash: {}\nField: {}\nDecoded text range: {}\n{}",
                clean_terminal(&hit.title),
                clean_terminal(&hit.kind),
                clean_terminal(&hit.project),
                clean_terminal(&hit.id),
                clean_terminal(&hit.kind),
                clean_terminal(&hit.role),
                origin,
                clean_terminal(&hit.path),
                hit.span[0],
                hit.span[1],
                clean_terminal(&hit.hash),
                clean_terminal(hit.field.as_deref().unwrap_or("-")),
                hit.text_span
                    .map(|span| format!("{}..{}", span[0], span[1]))
                    .unwrap_or_else(|| "-".to_owned()),
                clean_terminal(&hit.text),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

pub(crate) fn clean_terminal(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' | '\t' => ch,
            ch if ch.is_control() => '�',
            ch => ch,
        })
        .collect()
}

fn emit(json_output: bool, payload: Value, text: String) {
    if json_output {
        println!("{payload}");
    } else {
        println!("{}", clean_terminal(&text));
    }
}

fn clap_error(err: clap::Error) -> ExitCode {
    let json_output = std::env::args_os().skip(1).any(|arg| arg == "--json");
    if json_output && err.exit_code() != 0 {
        return fail(true, err);
    }
    let code = err.exit_code();
    let mut message = clean_terminal(&err.to_string());
    if !message.ends_with('\n') {
        message.push('\n');
    }
    if code != 0 {
        let _ = io::stderr().write_all(message.as_bytes());
    } else {
        let _ = io::stdout().write_all(message.as_bytes());
    }
    ExitCode::from(code as u8)
}

fn fail(json_output: bool, err: impl std::fmt::Display) -> ExitCode {
    if json_output {
        let payload = Fail {
            ok: false,
            error: err.to_string(),
        };
        println!(
            "{}",
            serde_json::to_string(&payload).expect("JSON failure payload")
        );
    } else {
        let message = clean_terminal(&err.to_string());
        let _ = writeln!(io::stderr(), "noesora: {message}");
    }
    ExitCode::from(2)
}

fn fail_search(json_output: bool, err: noesora_engine::SearchError) -> ExitCode {
    if json_output {
        let code = err.code();
        let payload = json!({
            "ok": false,
            "code": code,
            "error": err.to_string(),
        });
        println!("{payload}");
        ExitCode::from(2)
    } else {
        fail(false, err)
    }
}
