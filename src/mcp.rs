//! `noesora mcp`: stdio MCP server over the engine for the user-global default vault.
//!
//! Stdout carries protocol frames only; every diagnostic goes to stderr. The vault comes from
//! `~/.noesora/config.json` and never from the host's working directory.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use noesora_engine::index;
use noesora_engine::record::{self, RecordError, RecordStatus};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cli::{OkQuery, OkSearch};
use crate::config;

const INSTRUCTIONS: &str = "Noesora local vault. `search` returns cited records or an explicit \
refusal, with `truncated` indicating omitted ranked matches; `query` runs one read-only SQL SELECT \
over the index; `note` and `handoff` write durable records to the configured vault.";

const EMPTY_SECTION: &str = "None";

pub fn run() -> ExitCode {
    let vault = match config::home_dir().and_then(|home| config::load_default_vault(&home)) {
        Ok(vault) => vault,
        Err(err) => return fail(err),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return fail(err),
    };
    match runtime.block_on(serve(vault)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail(err),
    }
}

fn fail(err: impl std::fmt::Display) -> ExitCode {
    let _ = writeln!(io::stderr(), "noesora: {err}");
    ExitCode::from(2)
}

async fn serve(vault: PathBuf) -> Result<(), String> {
    let service = Noesora::new(vault)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|err| err.to_string())?;
    service.waiting().await.map_err(|err| err.to_string())?;
    Ok(())
}

#[derive(Clone)]
pub(crate) struct Noesora {
    vault: PathBuf,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    /// Words to look up in the vault's records.
    query: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct QueryArgs {
    /// One read-only SELECT or WITH statement over the index, for example
    /// `SELECT title, status FROM records`.
    sql: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct NoteArgs {
    /// One-line title.
    title: String,
    /// Note body.
    #[serde(default)]
    body: String,
    /// open (default), done, or blocked.
    #[serde(default)]
    status: Option<String>,
    /// Paths this note is about.
    #[serde(default)]
    files: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct HandoffArgs {
    /// One-line title.
    title: String,
    /// What the session set out to do.
    #[serde(default)]
    goal: String,
    /// What actually happened.
    #[serde(default)]
    outcome: String,
    /// Paths touched or relevant to the next session.
    #[serde(default)]
    files: Vec<String>,
    /// Approaches tried that did not work.
    #[serde(default)]
    dead_ends: Vec<String>,
    /// What the next session should do.
    #[serde(default)]
    next: Vec<String>,
    /// open (default), done, or blocked.
    #[serde(default)]
    status: Option<String>,
}

#[tool_router]
impl Noesora {
    pub(crate) fn new(vault: PathBuf) -> Self {
        Self {
            vault,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Search the vault's records. Returns cited hits (id, type, title, path, \
span, hash, text), `truncated` for omitted ranked hits, or an explicit refusal with reason \
no_evidence and `truncated: false`."
    )]
    async fn search(&self, Parameters(args): Parameters<SearchArgs>) -> CallToolResult {
        let result = self
            .blocking(move |root| {
                let found = index::search(root, &args.query).map_err(|err| err.to_string())?;
                structured(&OkSearch::from_result("search", found))
            })
            .await;
        respond(result)
    }

    #[tool(
        description = "Run one read-only SQL query against the vault index. Content is CSV; \
structured content has columns, rows and truncated. Writes are rejected."
    )]
    async fn query(&self, Parameters(args): Parameters<QueryArgs>) -> CallToolResult {
        let result = self
            .blocking(move |root| {
                let rows = index::query(root, &args.sql).map_err(|err| err.to_string())?;
                let csv = rows.csv();
                let value = serde_json::to_value(OkQuery::from_result(rows))
                    .map_err(|err| err.to_string())?;
                let mut result = CallToolResult::success(vec![ContentBlock::text(csv)]);
                result.structured_content = Some(value);
                Ok(result)
            })
            .await;
        respond(result)
    }

    #[tool(description = "Write a durable note record into the vault.")]
    async fn note(&self, Parameters(args): Parameters<NoteArgs>) -> CallToolResult {
        let result = self
            .blocking(move |root| {
                let status = parse_status(args.status.as_deref(), RecordStatus::Open)?;
                let written =
                    record::write_note(root, &args.title, &args.body, status, &args.files)
                        .map_err(|err| err.to_string())?;
                written_result("note", &written)
            })
            .await;
        respond(result)
    }

    #[tool(
        description = "Write a handoff record for the next session: goal, outcome, files, dead \
ends and next steps. Empty sections are stored as None."
    )]
    async fn handoff(&self, Parameters(args): Parameters<HandoffArgs>) -> CallToolResult {
        let result = self
            .blocking(move |root| {
                let status = parse_status(args.status.as_deref(), RecordStatus::Done)?;
                let files = clean_items(&args.files);
                let body = handoff_body(
                    &args.goal,
                    &args.outcome,
                    &files,
                    &args.dead_ends,
                    &args.next,
                );
                let written = record::write_handoff(root, &args.title, &body, status, &files)
                    .map_err(|err| err.to_string())?;
                written_result("handoff", &written)
            })
            .await;
        respond(result)
    }

    /// Runs engine work off the protocol thread, always against the configured vault. The vault
    /// is revalidated first so a vault removed mid-session can never resolve to a parent vault.
    async fn blocking<T, F>(&self, work: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&Path) -> Result<T, String> + Send + 'static,
    {
        let vault = self.vault.clone();
        tokio::task::spawn_blocking(move || {
            let root = config::validate_vault(&vault).map_err(|err| err.to_string())?;
            work(&root)
        })
        .await
        .map_err(|err| err.to_string())?
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Noesora {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
    }
}

fn respond(result: Result<CallToolResult, String>) -> CallToolResult {
    result.unwrap_or_else(|error| {
        CallToolResult::structured_error(json!({"ok": false, "error": error}))
    })
}

fn structured<T: Serialize>(payload: &T) -> Result<CallToolResult, String> {
    let value = serde_json::to_value(payload).map_err(|err| err.to_string())?;
    Ok(CallToolResult::structured(value))
}

fn written_result(
    command: &'static str,
    written: &record::WrittenNote,
) -> Result<CallToolResult, String> {
    structured(&json!({
        "ok": true,
        "command": command,
        "id": written.id,
        "path": written.path.display().to_string(),
        "vault": written.vault.display().to_string(),
    }))
}

fn parse_status(status: Option<&str>, default: RecordStatus) -> Result<RecordStatus, String> {
    let status = status.unwrap_or(match default {
        RecordStatus::Open => "open",
        RecordStatus::Done => "done",
        RecordStatus::Blocked => "blocked",
    });
    RecordStatus::parse(status).ok_or_else(|| RecordError::InvalidStatus.to_string())
}

fn clean_items(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|item| item.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|item| !item.is_empty())
        .collect()
}

fn prose_section(text: &str) -> String {
    match text.trim() {
        "" => EMPTY_SECTION.to_string(),
        text => text.to_string(),
    }
}

fn list_section(items: &[String]) -> String {
    let items = clean_items(items);
    if items.is_empty() {
        return EMPTY_SECTION.to_string();
    }
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Fixed section order so every handoff reads the same: Goal, Outcome, Files, Dead ends, Next.
fn handoff_body(
    goal: &str,
    outcome: &str,
    files: &[String],
    dead_ends: &[String],
    next: &[String],
) -> String {
    [
        ("Goal", prose_section(goal)),
        ("Outcome", prose_section(outcome)),
        ("Files", list_section(files)),
        ("Dead ends", list_section(dead_ends)),
        ("Next", list_section(next)),
    ]
    .iter()
    .map(|(heading, content)| format!("## {heading}\n\n{content}"))
    .collect::<Vec<_>>()
    .join("\n\n")
}
