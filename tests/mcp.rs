//! `noesora mcp` over real stdio pipes: raw JSON-RPC lines, no mocks, isolated `HOME`.

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

fn temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("noesora-mcp-{}-{nanos}-{seq}", std::process::id()));
    fs::create_dir(&path).expect("unique temp");
    path
}

fn bin(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_noesora"));
    cmd.env("HOME", home).env_remove("USERPROFILE");
    cmd
}

fn run_ok(cmd: &mut Command, label: &str) {
    let out = cmd.output().expect(label);
    assert!(
        out.status.success(),
        "{label} stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A vault at `base/<name>` that is also the configured global default under `base/home`.
struct World {
    base: PathBuf,
    home: PathBuf,
    default_vault: PathBuf,
    other_vault: PathBuf,
}

impl World {
    fn new() -> Self {
        let base = temp_dir();
        let home = base.join("home");
        let default_vault = base.join("default");
        let other_vault = base.join("other");
        for dir in [&home, &default_vault, &other_vault] {
            fs::create_dir_all(dir).unwrap();
        }
        for vault in [&default_vault, &other_vault] {
            run_ok(bin(&home).arg("init").current_dir(vault), "init");
        }
        run_ok(
            bin(&home)
                .args(["vault", "use"])
                .arg(&default_vault)
                .current_dir(&base),
            "vault use",
        );
        World {
            base,
            home,
            default_vault: fs::canonicalize(&default_vault).unwrap(),
            other_vault: fs::canonicalize(&other_vault).unwrap(),
        }
    }

    /// Host cwd is deliberately a different vault than the configured default.
    fn session(&self) -> Session {
        Session::start(&self.home, &self.other_vault, &[])
    }

    fn records(&self, vault: &Path) -> Vec<PathBuf> {
        let dir = vault.join("records");
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
        paths.sort();
        paths
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl Session {
    fn start(home: &Path, cwd: &Path, args: &[&str]) -> Self {
        let mut child = bin(home)
            .args(args)
            .arg("mcp")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn noesora mcp");
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut session = Session {
            child,
            stdin,
            lines,
            next_id: 1,
        };
        let init = session.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "noesora-test", "version": "0" }
            }),
        );
        assert!(
            init["result"]["capabilities"]["tools"].is_object(),
            "{init}"
        );
        session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        session
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// Every stdout line must be a JSON-RPC frame; anything else fails the test.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self
                .lines
                .recv_timeout(READ_TIMEOUT)
                .unwrap_or_else(|err| panic!("no response to {method}: {err}"));
            let frame: Value = serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("stdout is not protocol JSON ({err}): {line}"));
            assert_eq!(frame["jsonrpc"], "2.0", "not a JSON-RPC frame: {line}");
            if frame["id"] == id {
                return frame;
            }
        }
    }

    /// Returns the `tools/call` result object (tool-level errors included).
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let frame = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        assert!(frame.get("error").is_none(), "protocol error: {frame}");
        frame["result"].clone()
    }

    fn finish(mut self) -> String {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        assert!(status.success(), "mcp exited with {status}");
        let mut stderr = String::new();
        std::io::Read::read_to_string(&mut self.child.stderr.take().unwrap(), &mut stderr).unwrap();
        stderr
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().expect("text content")
}

#[test]
fn lists_exactly_the_four_tools() {
    let world = World::new();
    let mut session = world.session();
    let listed = session.request("tools/list", json!({}));
    let mut names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["handoff", "note", "query", "search"]);
    for tool in listed["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }
    session.finish();
}

#[test]
fn json_flag_adds_no_envelope_to_protocol_stdout() {
    let world = World::new();
    // `request` rejects any non-JSON-RPC stdout line, including a `{ok:...}` envelope.
    let mut session = Session::start(&world.home, &world.other_vault, &["--json"]);
    let listed = session.request("tools/list", json!({}));
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 4);
    session.finish();
}

#[test]
fn note_and_handoff_persist_in_global_vault_not_cwd() {
    let world = World::new();
    let mut session = world.session();

    let note = session.call(
        "note",
        json!({ "title": "Retry billing", "body": "Keep credits on the workspace.",
                "status": "done", "files": ["src/billing.rs"] }),
    );
    assert_eq!(note["isError"], false, "{note}");
    let written = &note["structuredContent"];
    assert_eq!(written["ok"], true);
    let note_path = PathBuf::from(written["path"].as_str().unwrap());
    assert_eq!(written["vault"], world.default_vault.to_str().unwrap());
    assert!(note_path.starts_with(&world.default_vault));
    let note_file = fs::read_to_string(&note_path).unwrap();
    assert!(
        note_file.contains("title: \"Retry billing\""),
        "{note_file}"
    );
    assert!(note_file.contains("status: done"), "{note_file}");
    assert!(note_file.contains("src/billing.rs"), "{note_file}");
    assert!(
        note_file.contains("Keep credits on the workspace."),
        "{note_file}"
    );

    let handoff = session.call(
        "handoff",
        json!({ "title": "Ship MCP", "goal": "Expose the vault over MCP",
                "files": ["src/mcp.rs", "tests/mcp.rs"], "next": ["Review", "Merge"] }),
    );
    assert_eq!(handoff["isError"], false, "{handoff}");
    let handoff_path = PathBuf::from(handoff["structuredContent"]["path"].as_str().unwrap());
    assert!(handoff_path.starts_with(&world.default_vault));
    let handoff_file = fs::read_to_string(&handoff_path).unwrap();
    assert!(handoff["structuredContent"]["id"]
        .as_str()
        .unwrap()
        .starts_with("hnd_"));
    assert!(handoff_file.contains("type: handoff\n"));
    assert!(handoff_file.contains("status: done\n"));
    let expected_body = "## Goal\n\nExpose the vault over MCP\n\n## Outcome\n\nNone\n\n\
## Files\n\n- src/mcp.rs\n- tests/mcp.rs\n\n## Dead ends\n\nNone\n\n## Next\n\n- Review\n- Merge";
    assert!(handoff_file.contains(expected_body), "{handoff_file}");
    assert!(handoff_file.contains("src/mcp.rs"), "{handoff_file}");

    // The host cwd vault stays untouched; the global default received both records.
    assert!(world.records(&world.other_vault).is_empty());
    assert_eq!(world.records(&world.default_vault).len(), 2);

    // Written records are immediately retrievable with citations.
    let found = session.call("search", json!({ "query": "credits" }));
    assert_eq!(found["structuredContent"]["refused"], false, "{found}");
    assert_eq!(found["structuredContent"]["truncated"], false, "{found}");
    let hit = &found["structuredContent"]["hits"][0];
    assert_eq!(hit["title"], "Retry billing");
    for field in ["id", "type", "path", "span", "hash", "text"] {
        assert!(hit[field].is_string(), "missing {field}: {hit}");
    }
    session.finish();
}

#[test]
fn note_and_handoff_report_invalid_input_as_tool_errors() {
    let world = World::new();
    let mut session = world.session();
    for (tool, args) in [
        ("note", json!({ "title": "x", "status": "weird" })),
        ("note", json!({ "title": "   " })),
        ("handoff", json!({ "title": "x", "status": "weird" })),
        ("handoff", json!({ "title": "" })),
    ] {
        let result = session.call(tool, args.clone());
        assert_eq!(result["isError"], true, "{tool} {args}: {result}");
        assert_eq!(result["structuredContent"]["ok"], false);
        assert!(result["structuredContent"]["error"].as_str().unwrap().len() > 1);
    }
    assert!(world.records(&world.default_vault).is_empty());
    session.finish();
}

#[test]
fn search_refuses_without_evidence() {
    let world = World::new();
    let mut session = world.session();
    session.call("note", json!({ "title": "Known", "body": "alpha beta" }));
    let refused = session.call("search", json!({ "query": "zzzunknownzzz" }));
    assert_eq!(refused["isError"], false, "{refused}");
    assert_eq!(
        refused["structuredContent"],
        json!({ "ok": true, "command": "search", "hits": [],
                "refused": true, "reason": "no_evidence", "truncated": false })
    );
    session.finish();
}

#[test]
fn search_reports_truncation_at_the_hit_cap() {
    let world = World::new();
    let mut session = world.session();
    for index in 0..11 {
        let title = format!("Cap fixture {index}");
        let body = if index < 10 {
            "exactcap overcap"
        } else {
            "overcap"
        };
        let written = session.call("note", json!({ "title": title, "body": body }));
        assert_eq!(written["isError"], false, "{written}");
    }

    let exact = session.call("search", json!({ "query": "exactcap" }));
    assert_eq!(
        exact["structuredContent"]["hits"].as_array().unwrap().len(),
        10
    );
    assert_eq!(exact["structuredContent"]["truncated"], false);

    let capped = session.call("search", json!({ "query": "overcap" }));
    assert_eq!(
        capped["structuredContent"]["hits"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert_eq!(capped["structuredContent"]["truncated"], true);
    session.finish();
}

#[test]
fn query_returns_csv_and_structured_rows_and_rejects_writes() {
    let world = World::new();
    let mut session = world.session();
    session.call("note", json!({ "title": "Row one", "status": "blocked" }));

    let rows = session.call(
        "query",
        json!({ "sql": "SELECT title, status FROM records" }),
    );
    assert_eq!(rows["isError"], false, "{rows}");
    assert_eq!(text(&rows), "title,status\nRow one,blocked\n");
    let structured = &rows["structuredContent"];
    assert_eq!(structured["columns"], json!(["title", "status"]));
    assert_eq!(structured["rows"], json!([["Row one", "blocked"]]));
    assert_eq!(structured["truncated"], false);

    let before = world.records(&world.default_vault);
    for sql in ["DELETE FROM records", "DROP TABLE records"] {
        let rejected = session.call("query", json!({ "sql": sql }));
        assert_eq!(rejected["isError"], true, "{sql}: {rejected}");
        assert_eq!(rejected["structuredContent"]["ok"], false);
    }
    assert_eq!(world.records(&world.default_vault), before);
    let after = session.call(
        "query",
        json!({ "sql": "SELECT count(*) AS total FROM records" }),
    );
    assert_eq!(text(&after), "total\n1\n");
    session.finish();
}

#[test]
fn missing_config_exits_nonzero_without_cwd_fallback() {
    let base = temp_dir();
    let home = base.join("home");
    let cwd_vault = base.join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cwd_vault).unwrap();
    run_ok(bin(&home).arg("init").current_dir(&cwd_vault), "init");

    for args in [&["mcp"][..], &["--json", "mcp"][..]] {
        let out = bin(&home)
            .args(args)
            .current_dir(&cwd_vault)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            out.stdout.is_empty(),
            "stdout must stay protocol-only: {:?}",
            out.stdout
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("no default vault"), "{stderr}");
    }
    assert!(!cwd_vault.join("records").exists());
    let _ = fs::remove_dir_all(base);
}

#[test]
fn invalid_config_exits_nonzero_with_visible_reason() {
    let base = temp_dir();
    let home = base.join("home");
    let cwd_vault = base.join("project");
    let not_a_vault = base.join("plain");
    fs::create_dir_all(home.join(".noesora")).unwrap();
    fs::create_dir_all(&cwd_vault).unwrap();
    fs::create_dir_all(&not_a_vault).unwrap();
    run_ok(bin(&home).arg("init").current_dir(&cwd_vault), "init");
    let config = home.join(".noesora").join("config.json");

    let cases = [
        ("not json".to_string(), "invalid config"),
        (
            r#"{"default_vault":"relative/vault"}"#.to_string(),
            "not an absolute path",
        ),
        (
            json!({ "default_vault": not_a_vault.to_str().unwrap() }).to_string(),
            "no vault at",
        ),
        (
            json!({ "default_vault": base.join("gone").to_str().unwrap() }).to_string(),
            "could not access",
        ),
    ];
    for (contents, expected) in cases {
        fs::write(&config, &contents).unwrap();
        let out = bin(&home)
            .arg("mcp")
            .current_dir(&cwd_vault)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{contents}");
        assert!(out.stdout.is_empty(), "{contents}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(expected), "{contents}: {stderr}");
    }
    assert!(!cwd_vault.join("records").exists());
    let _ = fs::remove_dir_all(base);
}

#[test]
fn parse_error_in_mcp_mode_keeps_json_envelope_off_stdout() {
    let world = World::new();
    let out = bin(&world.home)
        .args(["--json", "mcp", "unexpected"])
        .current_dir(&world.other_vault)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}
