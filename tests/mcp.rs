//! MCP stdio behavior through a real CLI process and JSON-RPC pipes.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
const READ_TIMEOUT: Duration = Duration::from_secs(20);

struct World {
    base: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
    session_id: String,
}

impl World {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let sequence = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "noesora-mcp-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&base).expect("create exclusive fixture root");
        let base = fs::canonicalize(base).expect("canonical fixture root");
        let home = base.join("home");
        let cwd = base.join("different-working-directory");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        run_ok(&mut bin(&home, &cwd).args(["init"]), "init");
        let session_path = base.join("session.jsonl");
        let fixture = omp_fixture(&session_path);
        let imported = bin(&home, &cwd)
            .args(["session", "import", "--client", "omp"])
            .arg(&session_path)
            .arg("--json")
            .output()
            .expect("import test session");
        assert!(
            imported.status.success(),
            "{}",
            String::from_utf8_lossy(&imported.stderr)
        );
        assert_eq!(fs::read(&session_path).unwrap(), fixture);
        let report: Value = serde_json::from_slice(&imported.stdout).expect("import report JSON");
        let session_id = report["session_id"]
            .as_str()
            .expect("session identifier")
            .to_owned();

        Self {
            base,
            home,
            cwd,
            session_id,
        }
    }

    fn mcp(&self, args: &[&str]) -> Session {
        Session::start(&self.home, &self.cwd, args)
    }
    fn import_session(
        &self,
        id: &str,
        project: &str,
        client: &str,
        user_text: &str,
        assistant_text: &str,
    ) -> String {
        let timestamp = "2026-10-21T08:00:00.000Z";
        let path = self.base.join(format!("{id}.jsonl"));
        let events = match client {
            "omp" => vec![
                json!({"type":"session","version":3,"id":id,"timestamp":timestamp,"cwd":project}),
                json!({"type":"message","id":format!("{id}-user"),"timestamp":timestamp,"message":{"role":"user","content":[{"type":"text","text":user_text}]}}),
                json!({"type":"message","id":format!("{id}-assistant"),"timestamp":timestamp,"message":{"role":"assistant","content":[{"type":"text","text":assistant_text}]}}),
            ],
            "claude-code" => vec![
                json!({"type":"user","sessionId":id,"timestamp":timestamp,"cwd":project,"message":{"role":"user","content":[{"type":"text","text":user_text}]}}),
                json!({"type":"assistant","sessionId":id,"timestamp":timestamp,"cwd":project,"message":{"role":"assistant","content":[{"type":"text","text":assistant_text}]}}),
            ],
            "codex" => vec![
                json!({"type":"session_meta","payload":{"id":id,"timestamp":timestamp,"cwd":project}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"text","text":user_text}]}}),
                json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"text","text":assistant_text}]}}),
            ],
            other => panic!("unsupported import fixture client {other}"),
        };
        let fixture = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
        fs::write(&path, &fixture).expect("write isolated source session");
        let imported = bin(&self.home, &self.cwd)
            .args(["session", "import", "--client"])
            .arg(client)
            .arg(&path)
            .arg("--json")
            .output()
            .expect("import isolated source session");
        assert!(
            imported.status.success(),
            "fixture import failed: {}",
            String::from_utf8_lossy(&imported.stderr)
        );
        assert_eq!(fs::read(&path).unwrap(), fixture);
        let report: Value = serde_json::from_slice(&imported.stdout).expect("import report JSON");
        assert_eq!(report["project"], project);
        report["session_id"]
            .as_str()
            .expect("imported session ID")
            .to_owned()
    }

    fn assert_no_records(&self) {
        let mut owner = self.mcp(&[]);
        let records = owner.call(
            "query",
            json!({"sql":"SELECT COUNT(*) AS count FROM records"}),
        );
        assert_eq!(records["isError"], false, "{records}");
        assert_eq!(records["structuredContent"]["rows"], json!([["0"]]));
        owner.finish();
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn bin(home: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_noesora"));
    command
        .env("NOESORA_HOME", home)
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .current_dir(cwd);
    command
}

fn run_ok(command: &mut Command, label: &str) {
    let output = command
        .output()
        .unwrap_or_else(|err| panic!("{label}: {err}"));
    assert!(
        output.status.success(),
        "{label} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn omp_fixture(path: &Path) -> Vec<u8> {
    let timestamp = "2026-10-21T08:00:00.000Z";
    let events = [
        json!({"type":"session","version":3,"id":"mcp-session","timestamp":timestamp,"cwd":"/projects/mcp"}),
        json!({"type":"message","id":"u1","timestamp":timestamp,"message":{"role":"user","content":[{"type":"text","text":"Where should application secrets be stored?"}]}}),
        json!({"type":"message","id":"a1","timestamp":timestamp,"message":{"role":"assistant","content":[{"type":"text","text":"Use a local operating system credential store for application secrets."}]}}),
    ];
    let bytes = events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    fs::write(path, &bytes).expect("write OMP session");
    bytes
}

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl Session {
    fn start(home: &Path, cwd: &Path, args: &[&str]) -> Self {
        let mut child = bin(home, cwd)
            .arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn noesora mcp");
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut session = Self {
            stdin: child.stdin.take(),
            child,
            lines,
            next_id: 1,
        };
        let initialized = session.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name":"noesora-integration-test","version":"1"}
            }),
        );
        assert!(
            initialized["result"]["capabilities"]["tools"].is_object(),
            "{initialized}"
        );
        session.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        session
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("MCP stdin open");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let line = self
                .lines
                .recv_timeout(READ_TIMEOUT)
                .unwrap_or_else(|err| panic!("no response to {method}: {err}"));
            let frame: Value = serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("MCP stdout was not protocol JSON ({err}): {line}"));
            assert_eq!(frame["jsonrpc"], "2.0", "unexpected stdout frame: {line}");
            if frame["id"] == id {
                return frame;
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let frame = self.request("tools/call", json!({"name":name,"arguments":arguments}));
        assert!(frame.get("error").is_none(), "MCP protocol error: {frame}");
        frame["result"].clone()
    }

    fn assert_denied(&mut self, name: &str, arguments: Value) {
        let frame = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        assert!(
            frame["error"].is_object() || frame["result"]["isError"] == true,
            "managed call to {name} was not denied: {frame}"
        );
    }

    fn finish(mut self) -> String {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        assert!(status.success(), "MCP exited with {status}");
        loop {
            match self.lines.recv_timeout(READ_TIMEOUT) {
                Ok(line) => {
                    let frame: Value = serde_json::from_str(&line).unwrap_or_else(|err| {
                        panic!("MCP stdout was not protocol JSON ({err}): {line}")
                    });
                    assert_eq!(frame["jsonrpc"], "2.0", "unexpected stdout frame: {line}");
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("MCP stdout did not close after the process exited")
                }
            }
        }
        let mut stderr = String::new();
        std::io::Read::read_to_string(&mut self.child.stderr.take().unwrap(), &mut stderr).unwrap();
        stderr
    }
}

#[test]
fn advertises_global_search_query_notes_handoffs_and_session_read_tools() {
    let world = World::new();
    let mut session = world.mcp(&[]);
    let listed = session.request("tools/list", json!({}));
    let names = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    for expected in [
        "search",
        "query",
        "note",
        "handoff",
        "session_list",
        "session_read",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    for tool in listed["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }
    session.finish();
}

#[test]
fn json_flag_keeps_mcp_stdout_protocol_only() {
    let world = World::new();
    let mut session = world.mcp(&["--json"]);
    let listed = session.request("tools/list", json!({}));
    assert!(listed["result"]["tools"].as_array().unwrap().len() >= 6);
    session.finish();
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_home_fails_before_mcp_startup_without_owner_records() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let sequence = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!(
        "noesora-mcp-nonutf8-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    fs::create_dir(&base).expect("create exclusive fixture root");
    let base = fs::canonicalize(base).expect("canonical fixture root");
    let home = base.join(OsString::from_vec(b"home-\xFF".to_vec()));
    let cwd = base.join("cwd");
    fs::create_dir(&cwd).expect("create working directory");

    let output = bin(&home, &cwd)
        .args(["mcp", "--json"])
        .stdin(Stdio::null())
        .output()
        .expect("run MCP with non-UTF-8 home");
    assert!(
        !output.status.success(),
        "MCP startup unexpectedly succeeded"
    );
    assert!(
        output.stdout.is_empty(),
        "startup diagnostic reached stdout"
    );
    let stderr = String::from_utf8(output.stderr).expect("startup diagnostic is UTF-8");
    assert!(stderr.starts_with("noesora: "), "{stderr:?}");
    assert!(
        !stderr.to_ascii_lowercase().contains("panicked"),
        "MCP startup panicked: {stderr:?}"
    );

    let store = noesora_engine::Store::open(&home).expect("open the failed startup store");
    assert_eq!(
        store.status().expect("read public record status").records,
        0,
        "failed MCP startup wrote an owner record"
    );
    fs::remove_dir_all(&base).expect("remove fixture root");
}

#[test]
fn tools_share_global_search_and_paged_raw_session_history_across_cwd() {
    let world = World::new();
    let mut session = world.mcp(&[]);

    let found = session.call(
        "search",
        json!({"query":"application secrets","mode":"text"}),
    );
    assert_eq!(found["isError"], false, "{found}");
    let evidence = &found["structuredContent"];
    assert_eq!(evidence["ok"], true);
    assert_eq!(evidence["refused"], false);
    assert_eq!(evidence["mode"], "text");
    let hit = &evidence["hits"][0];
    for field in [
        "id",
        "type",
        "title",
        "project",
        "client",
        "session_id",
        "path",
        "span",
        "hash",
        "text",
    ] {
        assert!(
            !hit[field].is_null(),
            "missing citation field {field}: {hit}"
        );
    }
    assert_eq!(hit["session_id"], world.session_id);

    let listing = session.call("session_list", json!({"offset":0,"limit":5}));
    assert_eq!(listing["isError"], false, "{listing}");
    let sessions = listing["structuredContent"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], world.session_id);
    assert_eq!(sessions[0]["project"], "/projects/mcp");

    let page = session.call(
        "session_read",
        json!({"id":world.session_id,"offset":1,"limit":1}),
    );
    assert_eq!(page["isError"], false, "{page}");
    assert_eq!(page["structuredContent"]["session"]["id"], world.session_id);
    assert_eq!(page["structuredContent"]["offset"], 1);
    assert_eq!(
        page["structuredContent"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let refused = session.call("search", json!({"query":"no-matching-evidence-token"}));
    assert_eq!(refused["isError"], false);
    assert_eq!(refused["structuredContent"]["refused"], true);
    assert_eq!(refused["structuredContent"]["reason"], "no_evidence");
    assert_eq!(refused["structuredContent"]["truncated"], false);
    session.finish();
}

#[test]
fn approved_note_and_handoff_use_global_records_and_queries_are_read_only() {
    let world = World::new();
    let mut session = world.mcp(&[]);

    let note = session.call(
        "note",
        json!({
            "title":"Credential policy",
            "body":"Keep service tokens in the operating system credential store.",
            "status":"done",
            "files":["src/auth.rs"],
            "project":"security"
        }),
    );
    assert_eq!(note["isError"], false, "{note}");
    let written = &note["structuredContent"];
    assert_eq!(written["ok"], true);
    assert_eq!(written["command"], "note");
    assert_eq!(written["kind"], "note");
    assert!(PathBuf::from(written["path"].as_str().unwrap()).starts_with(world.home.join("store")));

    let handoff = session.call(
        "handoff",
        json!({
            "title":"Next security review",
            "goal":"Review token handling",
            "outcome":"Store credentials in the operating system",
            "files":["src/auth.rs"],
            "dead_ends":["Repository-local secrets"],
            "next":["Confirm platform keychain behavior"],
            "status":"blocked",
            "project":"security"
        }),
    );
    assert_eq!(handoff["isError"], false, "{handoff}");
    assert_eq!(handoff["structuredContent"]["kind"], "handoff");
    assert!(
        PathBuf::from(handoff["structuredContent"]["path"].as_str().unwrap())
            .starts_with(world.home.join("store"))
    );

    let search = session.call(
        "search",
        json!({"query":"service tokens","project":"security"}),
    );
    assert_eq!(search["structuredContent"]["refused"], false, "{search}");
    assert_eq!(search["structuredContent"]["hits"][0]["type"], "note");

    let query = session.call("query", json!({"sql":"SELECT 1 AS value"}));
    assert_eq!(query["isError"], false, "{query}");
    assert_eq!(query["structuredContent"]["columns"], json!(["value"]));
    assert_eq!(query["structuredContent"]["rows"], json!([["1"]]));

    let rejected = session.call("query", json!({"sql":"DELETE FROM records"}));
    assert_eq!(rejected["isError"], true, "{rejected}");
    assert_eq!(rejected["structuredContent"]["ok"], false);

    let preserved = session.call(
        "search",
        json!({"query":"service tokens","project":"security"}),
    );
    assert_eq!(
        preserved["structuredContent"]["refused"], false,
        "{preserved}"
    );
    assert_eq!(preserved["structuredContent"]["hits"][0]["type"], "note");

    for (tool, args) in [
        ("note", json!({"title":"bad note","status":"invalid"})),
        ("handoff", json!({"title":"bad handoff","status":"invalid"})),
    ] {
        let error = session.call(tool, args);
        assert_eq!(error["isError"], true, "{tool}: {error}");
        assert_eq!(error["structuredContent"]["ok"], false);
        assert!(error["structuredContent"]["error"].is_string());
    }
    session.finish();
}

#[test]
fn owner_mcp_defaults_omitted_write_projects_and_rejects_explicit_empty() {
    let world = World::new();
    let mut session = world.mcp(&[]);

    let note = session.call(
        "note",
        json!({"title":"Default project note","body":"An omitted project uses local."}),
    );
    assert_eq!(note["isError"], false, "{note}");
    assert_eq!(note["structuredContent"]["project"], "local");

    let handoff = session.call(
        "handoff",
        json!({"title":"Default project handoff","goal":"Check the local default."}),
    );
    assert_eq!(handoff["isError"], false, "{handoff}");
    assert_eq!(handoff["structuredContent"]["project"], "local");

    for (tool, args) in [
        ("note", json!({"title":"Empty project note","project":""})),
        (
            "handoff",
            json!({"title":"Empty project handoff","project":""}),
        ),
    ] {
        let rejected = session.call(tool, args);
        assert_eq!(rejected["isError"], true, "{tool}: {rejected}");
        assert!(rejected["structuredContent"]["error"].is_string());
    }

    let count = session.call(
        "query",
        json!({"sql":"SELECT COUNT(*) AS count FROM records WHERE project='local'"}),
    );
    assert_eq!(count["structuredContent"]["rows"], json!([["2"]]));
    session.finish();
}

#[test]
fn managed_client_filters_use_canonical_ids_and_known_source_evidence() {
    let world = World::new();
    let mut known_sessions = Vec::new();
    for (client, id) in [
        ("omp", "filter-omp-session"),
        ("claude-code", "filter-claude-session"),
        ("codex", "filter-codex-session"),
    ] {
        let session_id = world.import_session(
            id,
            "/projects/mcp",
            client,
            "known client source filter marker",
            "An unrelated assistant response.",
        );
        known_sessions.push((client, session_id));
    }
    let mut session = world.mcp(&["--managed", "--project", "/projects/mcp"]);

    for client in [
        "omp",
        "claude-code",
        "codex",
        "grok",
        "opencode",
        "cursor-agent",
    ] {
        let filtered = session.call(
            "search",
            json!({
                "query":"known client source filter marker",
                "mode":"text",
                "client":client,
            }),
        );
        assert_eq!(filtered["isError"], false, "{client}: {filtered}");
        let body = &filtered["structuredContent"];
        assert_eq!(body["mode"], "text", "{client}: {filtered}");
        if let Some((_, expected_id)) = known_sessions.iter().find(|(source, _)| *source == client)
        {
            assert_eq!(body["refused"], false, "{client}: {filtered}");
            let hits = body["hits"].as_array().unwrap();
            assert!(hits
                .iter()
                .any(|hit| hit["session_id"] == expected_id.as_str()));
            assert!(hits
                .iter()
                .all(|hit| { hit["client"] == client && hit["project"] == "/projects/mcp" }));
        } else {
            assert_eq!(body["refused"], true, "{client}: {filtered}");
            assert_eq!(body["reason"], "no_evidence", "{client}: {filtered}");
            assert_eq!(body["hits"], json!([]), "{client}: {filtered}");
        }
    }
    session.finish();
}

#[test]
fn managed_project_connection_limits_search_and_direct_reads_to_exact_project() {
    let world = World::new();
    let other_session = world.import_session(
        "other-project-session",
        "/projects/other",
        "omp",
        "application secrets FOREIGN_PRIVATE_SESSION_MARKER",
        "Store application secrets outside the repository with FOREIGN_ASSISTANT_MARKER.",
    );
    let mut session = world.mcp(&["--managed", "--project", "/projects/mcp"]);

    let listed = session.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3, "managed tools: {tools:?}");
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    for expected in ["search", "session_list", "session_read"] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }

    let search = session.call("search", json!({"query":"application secrets"}));
    assert_eq!(search["isError"], false, "{search}");
    let hits = search["structuredContent"]["hits"].as_array().unwrap();
    assert!(hits.iter().any(|hit| hit["session_id"] == world.session_id));
    assert!(hits.iter().all(|hit| hit["project"] == "/projects/mcp"));

    let listing = session.call("session_list", json!({"limit":10}));
    assert_eq!(listing["isError"], false, "{listing}");
    let sessions = listing["structuredContent"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], world.session_id);
    assert_eq!(sessions[0]["project"], "/projects/mcp");

    let page = session.call(
        "session_read",
        json!({"id":world.session_id,"offset":0,"limit":10}),
    );
    assert_eq!(page["isError"], false, "{page}");
    assert_eq!(
        page["structuredContent"]["session"]["project"],
        "/projects/mcp"
    );
    let messages = page["structuredContent"]["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["text"]
            .as_str()
            .is_some_and(|text| text.contains("Use a local operating system credential store"))
    }));

    let widened_search = session.call(
        "search",
        json!({"query":"application secrets","project":"/projects/other"}),
    );
    assert_eq!(widened_search["isError"], true, "{widened_search}");
    let widened_list = session.call("session_list", json!({"project":"/projects/other"}));
    assert_eq!(widened_list["isError"], true, "{widened_list}");

    for id in [other_session.as_str(), "unknown-managed-session"] {
        let denied = session.call("session_read", json!({"id":id,"offset":0,"limit":10}));
        assert_eq!(denied["isError"], true, "{id}: {denied}");
        assert_eq!(
            denied["structuredContent"]["error"],
            "session is not available in this managed memory scope"
        );
        let response = denied.to_string();
        assert!(!response.contains("FOREIGN_PRIVATE_SESSION_MARKER"));
        assert!(!response.contains("FOREIGN_ASSISTANT_MARKER"));
    }

    session.assert_denied("query", json!({"sql":"SELECT 1"}));
    session.assert_denied("note", json!({"title":"Denied note","body":"no write"}));
    session.assert_denied(
        "handoff",
        json!({"title":"Denied handoff","goal":"no write"}),
    );
    session.finish();
    world.assert_no_records();
}

#[test]
fn managed_all_projects_is_explicit_read_only_and_can_read_both_projects() {
    let world = World::new();
    let other_session = world.import_session(
        "all-project-session",
        "/projects/other",
        "omp",
        "application secrets",
        "Store application secrets outside the repository.",
    );
    let mut session = world.mcp(&["--json", "--managed", "--all-projects"]);

    let listed = session.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3, "managed tools: {tools:?}");
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    for expected in ["search", "session_list", "session_read"] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }

    let listing = session.call("session_list", json!({"limit":10}));
    assert_eq!(listing["isError"], false, "{listing}");
    let sessions = listing["structuredContent"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions
        .iter()
        .any(|item| item["project"] == "/projects/mcp"));
    assert!(sessions
        .iter()
        .any(|item| item["project"] == "/projects/other"));

    let search = session.call("search", json!({"query":"application secrets"}));
    assert_eq!(search["isError"], false, "{search}");
    let hits = search["structuredContent"]["hits"].as_array().unwrap();
    assert!(hits.iter().any(|hit| hit["project"] == "/projects/mcp"));
    assert!(hits.iter().any(|hit| hit["project"] == "/projects/other"));

    let page = session.call(
        "session_read",
        json!({"id":other_session,"offset":0,"limit":10}),
    );
    assert_eq!(page["isError"], false, "{page}");
    assert_eq!(page["structuredContent"]["session"]["id"], other_session);
    let messages = page["structuredContent"]["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["text"]
            .as_str()
            .is_some_and(|text| text.contains("Store application secrets outside the repository"))
    }));

    session.assert_denied("query", json!({"sql":"SELECT id,project FROM sessions"}));
    session.assert_denied("note", json!({"title":"Denied note","body":"no write"}));
    session.assert_denied(
        "handoff",
        json!({"title":"Denied handoff","goal":"no write"}),
    );
    session.finish();
    world.assert_no_records();
}
