//! MCP stdio adapter over the user-global Noesora store.
//!
//! Stdout is reserved for MCP protocol frames; startup failures and diagnostics go to stderr.

use std::io::{self, Write};
use std::process::ExitCode;
use std::str::FromStr;

use noesora_engine::{
    Client, ContentScope, RecordInput, SearchFilters, SearchMode, SearchResponse, Store,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{config, memory_policy::MemoryPolicy};

const OWNER_INSTRUCTIONS: &str = "Noesora local archive. Search returns source-cited evidence or an explicit no-evidence refusal; query is read-only; notes and handoffs write approved records; session tools read raw imported history. Owner connections use the global NOESORA_HOME store.";
const PROJECT_READ_ONLY_INSTRUCTIONS: &str = "Noesora managed MCP connection. Read-only access is limited to the exact configured project. Only search, session_list, and session_read are available; SQL and approved-record writes are disabled. All tools use the global NOESORA_HOME store.";
const ALL_READ_ONLY_INSTRUCTIONS: &str = "Noesora managed MCP connection. Read-only access spans all projects only because this connection explicitly selected --all-projects. Only search, session_list, and session_read are available; SQL and approved-record writes are disabled. All tools use the global NOESORA_HOME store.";
const EMPTY_SECTION: &str = "None";

pub fn run(policy: MemoryPolicy) -> ExitCode {
    let store = match config::open_store() {
        Ok(store) => store,
        Err(err) => return fail(err),
    };
    if let Err(err) = config::require_json_safe_store(&store) {
        return fail(err);
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return fail(err),
    };
    match runtime.block_on(serve(store, policy)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail(err),
    }
}

fn fail(err: impl std::fmt::Display) -> ExitCode {
    let message = crate::cli::clean_terminal(&err.to_string());
    let _ = writeln!(io::stderr(), "noesora: {message}");
    ExitCode::from(2)
}

async fn serve(store: Store, policy: MemoryPolicy) -> Result<(), String> {
    let service = Noesora::new(store, policy)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|err| err.to_string())?;
    service.waiting().await.map_err(|err| err.to_string())?;
    Ok(())
}

#[derive(Clone)]
pub(crate) struct Noesora {
    store: Store,
    policy: MemoryPolicy,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    /// Query text.
    query: String,
    /// text (default) or semantic.
    #[serde(default = "default_search_mode")]
    mode: String,
    /// Optional project filter; managed project mode rejects filters outside its exact scope.
    #[serde(default)]
    project: Option<String>,
    /// Optional source client: omp, claude-code, codex, grok, opencode, or cursor-agent.
    #[serde(default)]
    client: Option<String>,
    /// conversation (default), tools, or all.
    #[serde(default = "default_search_scope")]
    scope: String,
    /// Maximum ranked hits, default 10.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct QueryArgs {
    /// One read-only SQL statement, for example SELECT count(*) FROM sessions.
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
    /// Optional project metadata.
    #[serde(default)]
    project: Option<String>,
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
    /// Optional project metadata.
    #[serde(default)]
    project: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SessionListArgs {
    /// Optional project filter; managed connections cannot widen their configured scope.
    #[serde(default)]
    project: Option<String>,
    /// Zero-based session offset.
    #[serde(default)]
    offset: usize,
    /// Maximum sessions to return.
    #[serde(default = "default_session_limit")]
    limit: usize,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SessionReadArgs {
    /// Stable session identifier returned by session_list or search.
    id: String,
    /// Zero-based message offset.
    #[serde(default)]
    offset: usize,
    /// Maximum messages to return.
    #[serde(default = "default_message_limit")]
    limit: usize,
}

fn default_search_mode() -> String {
    "text".to_owned()
}
fn default_search_scope() -> String {
    "conversation".to_owned()
}
fn default_session_limit() -> usize {
    50
}
fn default_message_limit() -> usize {
    40
}

#[tool_router]
impl Noesora {
    pub(crate) fn new(store: Store, policy: MemoryPolicy) -> Self {
        let mut tool_router = Self::tool_router();
        if policy.managed() {
            for name in ["query", "note", "handoff"] {
                tool_router.disable_route(name);
            }
        }
        Self {
            store,
            policy,
            tool_router,
        }
    }

    #[tool(
        description = "Search the archive within the connection's authorized project scope for cited evidence. Returns engine SearchHit fields (id, type, title, project, client, session_id, role, path, span, hash, text), refusal reason, index coverage and truncation state."
    )]
    async fn search(&self, Parameters(args): Parameters<SearchArgs>) -> CallToolResult {
        let policy = self.policy.clone();
        let result = self
            .blocking(move |store| {
                let project = policy.filter_project(args.project.as_deref())?;
                let mode = SearchMode::from_str(&args.mode)?;
                let scope = ContentScope::from_str(&args.scope)?;
                let client = args.client.as_deref().map(Client::from_str).transpose()?;
                let filters = SearchFilters {
                    project,
                    client,
                    scope,
                    limit: args.limit.unwrap_or(10),
                };
                let found = store
                    .search(&args.query, mode, &filters)
                    .map_err(|err| err.to_string())?;
                structured(&search_payload(found))
            })
            .await;
        respond(result)
    }

    #[tool(
        description = "Run one read-only SQL query against the global store. Text content is row-capped CSV; structured content has columns, rows and truncated."
    )]
    async fn query(&self, Parameters(args): Parameters<QueryArgs>) -> CallToolResult {
        if !self.policy.owner_tools() {
            return managed_tool_denied();
        }
        let result = self
            .blocking(move |store| {
                let rows = store.query(&args.sql).map_err(|err| err.to_string())?;
                let csv = rows.csv();
                let payload = with_command("query", rows);
                let mut response = CallToolResult::success(vec![ContentBlock::text(csv)]);
                response.structured_content = Some(payload);
                Ok(response)
            })
            .await;
        respond(result)
    }

    #[tool(description = "Write a durable approved note record into the global store.")]
    async fn note(&self, Parameters(args): Parameters<NoteArgs>) -> CallToolResult {
        if !self.policy.owner_tools() {
            return managed_tool_denied();
        }
        let result = self
            .blocking(move |store| {
                let status = parse_status(args.status.as_deref(), "open")?;
                let input = RecordInput {
                    title: args.title,
                    body: args.body,
                    kind: "note".to_owned(),
                    status,
                    project: args.project.unwrap_or_else(|| "local".to_owned()),
                    files: args.files,
                };
                let written = store.write_record(&input).map_err(|err| err.to_string())?;
                structured(&written_payload("note", written))
            })
            .await;
        respond(result)
    }

    #[tool(
        description = "Write a durable handoff record with fixed Goal, Outcome, Files, Dead ends, and Next sections. Empty sections are stored as None."
    )]
    async fn handoff(&self, Parameters(args): Parameters<HandoffArgs>) -> CallToolResult {
        if !self.policy.owner_tools() {
            return managed_tool_denied();
        }
        let result = self
            .blocking(move |store| {
                let status = parse_status(args.status.as_deref(), "done")?;
                let files = clean_items(&args.files);
                let body = handoff_body(
                    &args.goal,
                    &args.outcome,
                    &files,
                    &args.dead_ends,
                    &args.next,
                );
                let input = RecordInput {
                    title: args.title,
                    body,
                    kind: "handoff".to_owned(),
                    status,
                    project: args.project.unwrap_or_else(|| "local".to_owned()),
                    files,
                };
                let written = store.write_record(&input).map_err(|err| err.to_string())?;
                structured(&written_payload("handoff", written))
            })
            .await;
        respond(result)
    }

    #[tool(
        description = "List raw session history within the connection's authorized project scope, paged by session offset."
    )]
    async fn session_list(&self, Parameters(args): Parameters<SessionListArgs>) -> CallToolResult {
        let policy = self.policy.clone();
        let result = self
            .blocking(move |store| {
                let project = policy.filter_project(args.project.as_deref())?;
                let sessions = store
                    .sessions(project.as_deref(), args.offset, args.limit)
                    .map_err(|err| err.to_string())?;
                structured(&json!({
                    "ok": true,
                    "command": "session list",
                    "sessions": sessions,
                    "offset": args.offset,
                    "limit": args.limit,
                }))
            })
            .await;
        respond(result)
    }

    #[tool(
        description = "Read one page of a raw session transcript within the connection's authorized project scope; this does not create an approved record."
    )]
    async fn session_read(&self, Parameters(args): Parameters<SessionReadArgs>) -> CallToolResult {
        let policy = self.policy.clone();
        let result = self
            .blocking(move |store| {
                let page =
                    read_session_for_policy(&store, &policy, &args.id, args.offset, args.limit)?;
                structured(&with_command("session read", page))
            })
            .await;
        respond(result)
    }

    /// Run engine work away from the MCP protocol executor.
    async fn blocking<T, F>(&self, work: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(Store) -> Result<T, String> + Send + 'static,
    {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || work(store))
            .await
            .map_err(|err| err.to_string())?
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Noesora {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            match &self.policy {
                MemoryPolicy::Owner => OWNER_INSTRUCTIONS,
                MemoryPolicy::ProjectReadOnly(_) => PROJECT_READ_ONLY_INSTRUCTIONS,
                MemoryPolicy::AllReadOnly => ALL_READ_ONLY_INSTRUCTIONS,
            },
        )
    }
}

fn respond(result: Result<CallToolResult, String>) -> CallToolResult {
    result.unwrap_or_else(|error| {
        CallToolResult::structured_error(json!({"ok": false, "error": error}))
    })
}
fn managed_tool_denied() -> CallToolResult {
    CallToolResult::structured_error(json!({
        "ok": false,
        "error": "this tool is unavailable in managed read-only mode",
    }))
}

fn read_session_for_policy(
    store: &Store,
    policy: &MemoryPolicy,
    id: &str,
    offset: usize,
    limit: usize,
) -> Result<noesora_engine::TranscriptPage, String> {
    const UNAVAILABLE: &str = "session is not available in this managed memory scope";

    match policy {
        MemoryPolicy::Owner => store
            .read_session(id, offset, limit)
            .map_err(|err| err.to_string()),
        MemoryPolicy::ProjectReadOnly(project) => store
            .read_session_in_project(id, project, offset, limit)
            .map_err(|_| UNAVAILABLE.to_owned()),
        MemoryPolicy::AllReadOnly => store
            .read_session(id, offset, limit)
            .map_err(|_| UNAVAILABLE.to_owned()),
    }
}

fn structured<T: Serialize>(payload: &T) -> Result<CallToolResult, String> {
    let value = serde_json::to_value(payload).map_err(|err| err.to_string())?;
    Ok(CallToolResult::structured(value))
}

fn with_command<T: Serialize>(command: &'static str, payload: T) -> Value {
    match serde_json::to_value(payload) {
        Ok(Value::Object(mut object)) => {
            object.insert("ok".to_owned(), Value::Bool(true));
            object.insert("command".to_owned(), Value::String(command.to_owned()));
            Value::Object(object)
        }
        Ok(value) => json!({"ok": true, "command": command, "payload": value}),
        Err(err) => json!({"ok": false, "error": err.to_string()}),
    }
}

fn search_payload(response: SearchResponse) -> Value {
    let reason = if response.refused {
        Some(response.reason.unwrap_or_else(|| "no_evidence".to_owned()))
    } else {
        response.reason
    };
    json!({
        "ok": true,
        "command": "search",
        "hits": response.hits,
        "refused": response.refused,
        "reason": reason,
        "truncated": response.truncated,
        "mode": response.mode,
        "total_chunks": response.total_chunks,
        "embedded_chunks": response.embedded_chunks,
        "index_complete": response.index_complete,
    })
}

fn written_payload(command: &'static str, written: noesora_engine::WrittenRecord) -> Value {
    json!({
        "ok": true,
        "command": command,
        "id": written.id,
        "path": written.path,
        "kind": written.kind,
        "project": written.project,
    })
}

fn parse_status(status: Option<&str>, default: &'static str) -> Result<String, String> {
    let status = status.unwrap_or(default);
    match status {
        "open" | "done" | "blocked" => Ok(status.to_owned()),
        _ => Err(format!(
            "invalid record status {status:?}; use open, done or blocked"
        )),
    }
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
        "" => EMPTY_SECTION.to_owned(),
        text => text.to_owned(),
    }
}

fn list_section(items: &[String]) -> String {
    let items = clean_items(items);
    if items.is_empty() {
        return EMPTY_SECTION.to_owned();
    }
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Fixed section order keeps handoffs scannable by the next session.
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
