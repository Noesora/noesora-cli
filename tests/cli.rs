use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let sequence = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "noesora-cli-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create unique fixture root");
        Self(fs::canonicalize(path).expect("canonical fixture root"))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).expect("create fixture directory");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
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

fn output(command: &mut Command, label: &str) -> Output {
    command
        .output()
        .unwrap_or_else(|err| panic!("{label}: {err}"))
}

fn json_output(command: &mut Command, label: &str) -> (Output, Value) {
    let out = output(command, label);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "{label} did not produce JSON: {err}; stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, value)
}

fn assert_success(out: &Output, label: &str) {
    assert!(
        out.status.success(),
        "{label} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_json_error(out: &Output, value: &Value, label: &str) {
    assert_eq!(out.status.code(), Some(2), "{label}: {out:?}");
    assert_eq!(value["ok"], false, "{label}: {value}");
    assert!(
        value["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "{label}: {value}"
    );
}

fn omp_fixture(path: &Path, id: &str) -> Vec<u8> {
    let timestamp = "2026-10-21T08:00:00.000Z";
    let events = [
        json!({
            "type": "session",
            "version": 3,
            "id": id,
            "timestamp": timestamp,
            "cwd": "/projects/harbor"
        }),
        json!({
            "type": "message",
            "id": "user-1",
            "timestamp": timestamp,
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "Where should application access secrets belong?"}]
            }
        }),
        json!({
            "type": "message",
            "id": "assistant-1",
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Save access tokens in the operating system credential store."}]
            }
        }),
        json!({
            "type": "message",
            "id": "tool-1",
            "timestamp": timestamp,
            "message": {
                "role": "toolResult",
                "toolCallId": "call-1",
                "toolName": "bash",
                "content": [{"type": "text", "text": "local diagnostic output"}],
                "isError": false
            }
        }),
    ];
    let bytes = events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    fs::write(path, &bytes).expect("write OMP session fixture");
    bytes
}

#[test]
fn store_resolution_is_global_across_working_directories() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let first = fixture.dir("projects/first");
    let second = fixture.dir("projects/second");

    let (init, initialized) = json_output(&mut bin(&home, &first).args(["--json", "init"]), "init");
    assert_success(&init, "init");
    assert_eq!(initialized["ok"], true);
    assert_eq!(
        initialized["root"],
        home.join("store").display().to_string()
    );

    let (status, payload) = json_output(
        &mut bin(&home, &second).args(["status", "--json"]),
        "status",
    );
    assert_success(&status, "status from another cwd");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["root"], home.join("store").display().to_string());
    assert_eq!(payload["sessions"], 0);
    assert!(!first.join(".noesora").exists());
    assert!(!second.join(".noesora").exists());
}

#[test]
fn session_import_preserves_source_and_list_show_search_share_global_store() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let import_cwd = fixture.dir("importer");
    let read_cwd = fixture.dir("elsewhere");
    let source = fixture.path().join("session.jsonl");
    let original = omp_fixture(&source, "session-omp-a");

    let (init, _) = json_output(
        &mut bin(&home, &import_cwd).args(["init", "--json"]),
        "init",
    );
    assert_success(&init, "init");
    let (import, report) = json_output(
        &mut bin(&home, &import_cwd)
            .args(["session", "import", "--client", "omp"])
            .arg(&source)
            .arg("--json"),
        "session import",
    );
    assert_success(&import, "session import");
    assert_eq!(report["ok"], true);
    assert_eq!(report["client"], "omp");
    assert_eq!(report["project"], "/projects/harbor");
    assert_eq!(report["messages"], 3);
    assert_eq!(report["duplicate"], false);
    assert_eq!(
        fs::read(&source).unwrap(),
        original,
        "import must preserve source bytes"
    );

    let session_id = report["session_id"].as_str().expect("stable session id");
    let (list, listing) = json_output(
        &mut bin(&home, &read_cwd).args(["session", "list", "--json"]),
        "session list",
    );
    assert_success(&list, "session list from another cwd");
    assert_eq!(listing["ok"], true);
    assert_eq!(listing["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(listing["sessions"][0]["id"], session_id);
    assert_eq!(listing["sessions"][0]["client"], "omp");
    assert_eq!(listing["sessions"][0]["project"], "/projects/harbor");

    let (show, page) = json_output(
        &mut bin(&home, &read_cwd).args([
            "session", "show", session_id, "--offset", "1", "--limit", "1", "--json",
        ]),
        "session show",
    );
    assert_success(&show, "session show page");
    assert_eq!(page["ok"], true);
    assert_eq!(page["session"]["id"], session_id);
    assert_eq!(page["offset"], 1);
    assert_eq!(page["messages"].as_array().unwrap().len(), 1);
    assert!(page["messages"][0]["role"].is_string());

    let (search, found) = json_output(
        &mut bin(&home, &read_cwd).args([
            "search",
            "access secrets",
            "--mode",
            "text",
            "--project",
            "/projects/harbor",
            "--client",
            "omp",
            "--json",
        ]),
        "search",
    );
    assert_success(&search, "global text search");
    assert_eq!(found["ok"], true);
    assert_eq!(found["refused"], false);
    assert_eq!(found["mode"], "text");
    let hit = &found["hits"][0];
    for field in [
        "id", "type", "title", "project", "client", "path", "span", "hash", "text",
    ] {
        assert!(!hit[field].is_null(), "missing cited field {field}: {hit}");
    }
    assert_eq!(hit["project"], "/projects/harbor");
    assert_eq!(hit["client"], "omp");
    assert_eq!(hit["session_id"], session_id);

    let (refusal, no_evidence) = json_output(
        &mut bin(&home, &read_cwd).args(["search", "unknown-unrelated-phrase", "--json"]),
        "no evidence search",
    );
    assert_success(&refusal, "no evidence search");
    assert_eq!(no_evidence["refused"], true);
    assert_eq!(no_evidence["reason"], "no_evidence");
    assert_eq!(no_evidence["truncated"], false);
}

#[test]
fn native_claude_and_codex_history_imports_keep_their_source_files() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");

    let claude_path = fixture.path().join("claude.jsonl");
    let claude_events = [
        json!({
            "type": "user",
            "sessionId": "cli-claude-session",
            "cwd": "/workspace/claude",
            "uuid": "claude-user",
            "timestamp": "2026-10-21T08:00:00Z",
            "message": {"role": "user", "content": "Claude native import marker"}
        }),
        json!({
            "type": "assistant",
            "sessionId": "cli-claude-session",
            "cwd": "/workspace/claude",
            "uuid": "claude-assistant",
            "timestamp": "2026-10-21T08:01:00Z",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Claude response marker"}]
            }
        }),
    ];
    let claude_bytes = claude_events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    fs::write(&claude_path, &claude_bytes).expect("write Claude transcript");

    let codex_path = fixture.path().join("codex.jsonl");
    let codex_events = [
        json!({
            "type": "session_meta",
            "payload": {
                "id": "cli-codex-session",
                "timestamp": "2026-10-21T08:00:00Z",
                "cwd": "/workspace/codex"
            }
        }),
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Codex native import marker"}]
            }
        }),
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Codex response marker"}]
            }
        }),
    ];
    let codex_bytes = codex_events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    fs::write(&codex_path, &codex_bytes).expect("write Codex transcript");

    for (client, path, original) in [
        ("claude-code", &claude_path, &claude_bytes),
        ("codex", &codex_path, &codex_bytes),
    ] {
        let (import, report) = json_output(
            &mut bin(&home, &cwd)
                .args(["--json", "session", "import", "--client", client])
                .arg(path),
            client,
        );
        assert_success(&import, client);
        assert_eq!(report["ok"], true);
        assert_eq!(report["client"], client);
        assert_eq!(report["messages"], 2);
        assert_eq!(fs::read(path).unwrap().as_slice(), original.as_slice());
    }
}

#[test]
fn configured_sources_sync_only_after_explicit_addition() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");
    let source_root = fixture.dir("source");
    let source_file = source_root.join("one.jsonl");
    let original = omp_fixture(&source_file, "source-session-1");

    let (init, _) = json_output(&mut bin(&home, &cwd).args(["init", "--json"]), "init");
    assert_success(&init, "init");
    let (before, status) = json_output(&mut bin(&home, &cwd).args(["status", "--json"]), "status");
    assert_success(&before, "initial status");
    assert_eq!(status["sessions"], 0);

    let (empty_sync, empty_report) =
        json_output(&mut bin(&home, &cwd).args(["sync", "--json"]), "empty sync");
    assert_success(&empty_sync, "sync with no configured sources");
    assert_eq!(empty_report["ok"], true);
    assert_eq!(empty_report["imports"], json!([]));
    assert_eq!(empty_report["errors"], json!([]));
    assert_eq!(empty_report.as_object().unwrap().len(), 3);

    let (add, source) = json_output(
        &mut bin(&home, &cwd)
            .args(["source", "add", "--client", "omp"])
            .arg(&source_root)
            .arg("--json"),
        "source add",
    );
    assert_success(&add, "source add");
    assert_eq!(source["ok"], true);
    assert_eq!(source["enabled"], true);

    let (list, sources) = json_output(
        &mut bin(&home, &cwd).args(["source", "list", "--json"]),
        "source list",
    );
    assert_success(&list, "source list");
    assert_eq!(sources["sources"].as_array().unwrap().len(), 1);
    assert_eq!(
        sources["sources"][0]["path"],
        source_root.display().to_string()
    );

    let (sync, result) = json_output(&mut bin(&home, &cwd).args(["sync", "--json"]), "sync");
    assert_success(&sync, "root sync");
    assert_eq!(result["ok"], true);
    assert_eq!(result["imports"].as_array().unwrap().len(), 1);
    assert_eq!(result["errors"], json!([]));
    assert_eq!(result.as_object().unwrap().len(), 3);
    assert_eq!(fs::read(&source_file).unwrap(), original);

    let (legacy_sync, error) = json_output(
        &mut bin(&home, &cwd).args(["source", "sync", "--json"]),
        "removed source sync command",
    );
    assert_json_error(&legacy_sync, &error, "source sync is no longer a command");

    let (after, status) = json_output(&mut bin(&home, &cwd).args(["status", "--json"]), "status");
    assert_success(&after, "status after source sync");
    assert_eq!(status["sessions"], 1);
}

#[test]
fn readding_disabled_source_preserves_state_and_skips_sync() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");
    let source_root = fixture.dir("source");
    omp_fixture(&source_root.join("one.jsonl"), "disabled-source-session");

    let (created, source) = json_output(
        &mut bin(&home, &cwd)
            .args(["source", "add", "--client", "omp"])
            .arg(&source_root)
            .arg("--json"),
        "create source before disabling",
    );
    assert_success(&created, "create source before disabling");
    let manifest = home
        .join("store/sources")
        .join(format!("{}.json", source["id"].as_str().unwrap()));
    let mut manifest_value: Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).expect("read source manifest");
    manifest_value["enabled"] = json!(false);
    fs::write(&manifest, serde_json::to_vec(&manifest_value).unwrap())
        .expect("disable source manifest");

    let (readd_json, readded) = json_output(
        &mut bin(&home, &cwd)
            .args(["source", "add", "--client", "omp"])
            .arg(&source_root)
            .arg("--json"),
        "re-add disabled source as JSON",
    );
    assert_success(&readd_json, "re-add disabled source as JSON");
    assert_eq!(readded["enabled"], false);

    let readd_text = output(
        &mut bin(&home, &cwd)
            .args(["source", "add", "--client", "omp"])
            .arg(&source_root),
        "re-add disabled source as text",
    );
    assert_success(&readd_text, "re-add disabled source as text");
    let readd_text = String::from_utf8(readd_text.stdout).expect("UTF-8 source output");
    assert!(
        readd_text.to_ascii_lowercase().starts_with("disabled "),
        "source add should report the stored disabled state: {readd_text}"
    );
    let persisted: Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).expect("read source manifest");
    assert_eq!(persisted["enabled"], false);

    let (sync, report) = json_output(&mut bin(&home, &cwd).args(["sync", "--json"]), "sync");
    assert_success(&sync, "sync disabled source");
    assert_eq!(report["ok"], true);
    assert_eq!(report["imports"], json!([]));
    assert_eq!(report["errors"], json!([]));

    let (status, store_status) =
        json_output(&mut bin(&home, &cwd).args(["status", "--json"]), "status");
    assert_success(&status, "status after disabled source sync");
    assert_eq!(store_status["sessions"], 0);
}

#[cfg(unix)]
#[test]
fn hostile_source_path_is_sanitized_in_text_and_preserved_in_json() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");
    let source_root = fixture.dir("source-Ω\n\t-\u{1b}]8;;https://example.invalid\u{7}");

    let text = output(
        &mut bin(&home, &cwd)
            .args(["source", "add", "--client", "omp"])
            .arg(&source_root),
        "source add with terminal controls",
    );
    assert_success(&text, "source add with terminal controls");
    assert!(
        !text.stdout.contains(&0x1b),
        "text output contains an active ESC sequence: {:?}",
        text.stdout
    );
    assert!(
        !text.stdout.contains(&0x07),
        "text output contains BEL: {:?}",
        text.stdout
    );
    let text_stdout = String::from_utf8(text.stdout).expect("UTF-8 text output");
    assert!(text_stdout.contains("example.invalid"));
    assert!(text_stdout.contains("Ω\n\t-"));

    let (json_out, source) = json_output(
        &mut bin(&home, &cwd)
            .args(["source", "add", "--client", "omp"])
            .arg(&source_root)
            .arg("--json"),
        "source add with terminal controls as JSON",
    );
    assert_success(&json_out, "source add with terminal controls as JSON");
    assert_eq!(
        source["path"],
        source_root.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(source["enabled"], true);
    assert!(!json_out.stdout.contains(&0x1b));
    assert!(!json_out.stdout.contains(&0x07));
}

#[test]
fn root_sync_reports_partial_failure_and_keeps_successful_imports_global() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let sync_cwd = fixture.dir("workspace/sync");
    let read_cwd = fixture.dir("workspace/reader");
    let valid_source = fixture.dir("sources/valid");
    let malformed_source = fixture.dir("sources/malformed");
    let valid_file = valid_source.join("valid.jsonl");
    let original = omp_fixture(&valid_file, "partial-session-1");
    let malformed_file = malformed_source.join("malformed.jsonl");
    fs::write(&malformed_file, b"{malformed json\n").expect("write malformed source");

    let (init, _) = json_output(&mut bin(&home, &sync_cwd).args(["init", "--json"]), "init");
    assert_success(&init, "init");

    for (source_root, label) in [
        (&valid_source, "valid source"),
        (&malformed_source, "malformed source"),
    ] {
        let (add, source) = json_output(
            &mut bin(&home, &sync_cwd)
                .args(["source", "add", "--client", "omp"])
                .arg(source_root)
                .arg("--json"),
            label,
        );
        assert_success(&add, label);
        assert_eq!(source["ok"], true);
        assert_eq!(source["enabled"], true);
    }

    let (sync, partial) = json_output(
        &mut bin(&home, &sync_cwd).args(["sync", "--json"]),
        "partial root sync",
    );
    assert_eq!(sync.status.code(), Some(2), "partial root sync: {sync:?}");
    assert_eq!(partial["ok"], false);
    assert!(partial["error"]
        .as_str()
        .is_some_and(|error| !error.is_empty()));
    let fields = partial.as_object().expect("flat sync response");
    assert_eq!(fields.len(), 4);
    assert!(fields.contains_key("imports"));
    assert!(fields.contains_key("errors"));
    assert_eq!(partial["imports"].as_array().unwrap().len(), 1);
    let session_id = partial["imports"][0]["session_id"]
        .as_str()
        .expect("successful import remains in partial report");
    assert_eq!(partial["errors"].as_array().unwrap().len(), 1);
    let source_error = partial["errors"][0]
        .as_str()
        .expect("source-specific sync error");
    assert!(
        source_error.contains("malformed"),
        "sync error should identify the malformed source: {source_error}"
    );

    let (list, sessions) = json_output(
        &mut bin(&home, &read_cwd).args(["session", "list", "--json"]),
        "sessions after partial sync from another cwd",
    );
    assert_success(&list, "list globally imported session from another cwd");
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(sessions["sessions"][0]["id"], session_id);

    let (show, transcript) = json_output(
        &mut bin(&home, &read_cwd).args(["session", "show", session_id, "--json"]),
        "show session imported by partial sync from another cwd",
    );
    assert_success(&show, "show globally imported session from another cwd");
    assert_eq!(transcript["session"]["id"], session_id);
    assert!(transcript["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| {
            message["text"]
                .as_str()
                .is_some_and(|text| text.contains("operating system credential store"))
        }));
    assert_eq!(fs::read(&valid_file).unwrap(), original);
}

#[test]
fn explicit_vault_import_preserves_original_records_and_rebuilds_search_data() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");
    let legacy_project = fixture.path().join("legacy-project");
    let record_dir = fixture.dir("legacy-project/.noesora/records/notes");
    let source_record = record_dir.join("decision.md");
    let original = concat!(
        "---\n",
        "schema: noesora.record/v1\n",
        "id: legacy-decision\n",
        "type: note\n",
        "title: \"Legacy decision\"\n",
        "status: done\n",
        "created_at: 2026-10-21T08:00:00Z\n",
        "files: []\n",
        "---\n",
        "\n",
        "Legacy migration marker.\n",
    )
    .as_bytes()
    .to_vec();
    fs::write(&source_record, &original).expect("write legacy record");

    let (import, result) = json_output(
        &mut bin(&home, &cwd)
            .args(["--json", "import-vault"])
            .arg(&legacy_project),
        "explicit vault import",
    );
    assert_success(&import, "explicit vault import");
    assert_eq!(result["ok"], true);
    assert_eq!(result["command"], "import-vault");
    assert_eq!(result["imported"], 1);
    assert_eq!(fs::read(&source_record).unwrap(), original);

    let (rebuild, status) =
        json_output(&mut bin(&home, &cwd).args(["--json", "rebuild"]), "rebuild");
    assert_success(&rebuild, "rebuild migrated data");
    assert_eq!(status["ok"], true);
    assert_eq!(status["records"], 1);

    let (search, evidence) = json_output(
        &mut bin(&home, &cwd).args(["--json", "search", "legacy migration marker"]),
        "search rebuilt legacy record",
    );
    assert_success(&search, "search rebuilt legacy record");
    assert_eq!(evidence["refused"], false);
    assert_eq!(
        evidence["hits"][0]["project"],
        legacy_project.display().to_string()
    );
    assert_eq!(fs::read(&source_record).unwrap(), original);
}

#[test]
fn note_query_model_state_and_semantic_errors_use_engine_results() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("project");
    let init = bin(&home, &cwd).args(["init"]).output().unwrap();
    assert_success(&init, "init");

    let (note, written) = json_output(
        &mut bin(&home, &cwd).args([
            "note",
            "--json",
            "--title",
            "Credential policy",
            "--status",
            "done",
            "--project",
            "design",
            "--file",
            "src/auth.rs",
            "Use the system credential store.",
        ]),
        "note",
    );
    assert_success(&note, "note");
    assert_eq!(written["ok"], true);
    assert_eq!(written["kind"], "note");
    assert_eq!(written["project"], "design");
    assert!(written["path"].is_string());

    let (search, result) = json_output(
        &mut bin(&home, &cwd).args([
            "search",
            "credential store",
            "--project",
            "design",
            "--json",
        ]),
        "note search",
    );
    assert_success(&search, "search note");
    assert_eq!(result["refused"], false);
    assert_eq!(result["hits"][0]["type"], "note");
    assert_eq!(result["hits"][0]["project"], "design");

    let (query, result) = json_output(
        &mut bin(&home, &cwd).args(["query", "SELECT 1 AS value", "--json"]),
        "query",
    );
    assert_success(&query, "read-only query");
    assert_eq!(result["columns"], json!(["value"]));
    assert_eq!(result["rows"], json!([["1"]]));

    let (rejected, error) = json_output(
        &mut bin(&home, &cwd).args(["query", "DELETE FROM records", "--json"]),
        "rejected query",
    );
    assert_json_error(&rejected, &error, "write query rejection");

    let (model, model_state) = json_output(
        &mut bin(&home, &cwd).args(["model", "status", "--json"]),
        "model status",
    );
    assert_success(&model, "model status");
    assert_eq!(model_state["ok"], true);
    assert!(model_state["installed"].is_boolean());

    let (index, error) = json_output(
        &mut bin(&home, &cwd).args(["--json", "index"]),
        "index without installed model",
    );
    assert_json_error(&index, &error, "index without installed model");

    let (semantic, error) = json_output(
        &mut bin(&home, &cwd).args(["search", "credential", "--mode", "semantic", "--json"]),
        "semantic search without installed model",
    );
    assert_json_error(&semantic, &error, "semantic search without model");
}

#[test]
fn optional_note_project_uses_the_engine_local_default() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");

    let (note, written) = json_output(
        &mut bin(&home, &cwd).args([
            "--json",
            "note",
            "--title",
            "Unscoped note",
            "Project metadata was omitted.",
        ]),
        "note without an explicit project",
    );
    assert_success(&note, "note without an explicit project");
    assert_eq!(written["ok"], true);
    assert_eq!(written["project"], "local");
}

// macOS rejects this byte-path fixture before the CLI can serialize it.
#[cfg(target_os = "linux")]
#[test]
fn json_note_with_non_utf8_root_fails_before_persisting_or_indexing() {
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new();
    let home = fixture
        .path()
        .join(std::ffi::OsString::from_vec(b"home-\xFF".to_vec()));
    fs::create_dir(&home).expect("create non-UTF-8 store home");
    let cwd = fixture.dir("workspace");

    let (note, error) = json_output(
        &mut bin(&home, &cwd).args([
            "--json",
            "note",
            "--title",
            "Non-UTF-8 store note",
            "This must not persist.",
        ]),
        "note with non-UTF-8 store root",
    );
    assert_json_error(&note, &error, "reject note with non-UTF-8 store root");

    let store_root = home.join("store");
    let notes = store_root.join("records/notes");
    assert_eq!(
        fs::read_dir(&notes).expect("read note records").count(),
        0,
        "the failed JSON command must not persist a note"
    );
    assert!(
        !store_root.join("index.sqlite").exists(),
        "the failed JSON command must not build an index"
    );

    let status = output(&mut bin(&home, &cwd).args(["status"]), "text status");
    assert_success(&status, "text status after rejected note");
    let status_text = String::from_utf8(status.stdout).expect("UTF-8 text status");
    assert!(status_text.contains("0 records"), "{status_text}");
    assert!(status_text.contains("0 text chunks"), "{status_text}");
}

#[cfg(target_os = "linux")]
#[test]
fn json_source_add_with_non_utf8_path_does_not_register_source() {
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");
    let source_root = fixture
        .path()
        .join(std::ffi::OsString::from_vec(b"source-\xFF".to_vec()));
    fs::create_dir(&source_root).expect("create non-UTF-8 source root");
    let source_file = source_root.join("one.jsonl");
    let original = omp_fixture(&source_file, "non-utf8-source");

    let (add, error) = json_output(
        &mut bin(&home, &cwd)
            .args(["--json", "source", "add", "--client", "omp"])
            .arg(&source_root),
        "source add with non-UTF-8 path",
    );
    assert_json_error(&add, &error, "reject non-UTF-8 source path");
    assert_eq!(
        fs::read_dir(home.join("store/sources"))
            .expect("read configured sources")
            .count(),
        0,
        "failed source serialization must not publish a manifest"
    );

    let (list, sources) = json_output(
        &mut bin(&home, &cwd).args(["source", "list", "--json"]),
        "source list",
    );
    assert_success(&list, "source list after rejected add");
    assert_eq!(sources["sources"], json!([]));

    let (sync, report) = json_output(&mut bin(&home, &cwd).args(["sync", "--json"]), "sync");
    assert_success(&sync, "sync after rejected source add");
    assert_eq!(report["imports"], json!([]));
    assert_eq!(fs::read(&source_file).unwrap(), original);

    let (status, store_status) =
        json_output(&mut bin(&home, &cwd).args(["status", "--json"]), "status");
    assert_success(&status, "status after rejected source add");
    assert_eq!(store_status["sessions"], 0);
}

#[cfg(unix)]
#[test]
fn parser_diagnostics_sanitize_terminal_controls() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");
    let hostile_argument = "not-a-command-λ\u{1b}]8;;https://example.invalid\u{7}";

    let diagnostic = output(
        &mut bin(&home, &cwd).arg(hostile_argument),
        "parser diagnostic with terminal controls",
    );
    assert_eq!(diagnostic.status.code(), Some(2));
    assert!(
        !diagnostic.stderr.contains(&0x1b),
        "diagnostic contains an active ESC sequence: {:?}",
        diagnostic.stderr
    );
    assert!(
        !diagnostic.stderr.contains(&0x07),
        "diagnostic contains BEL: {:?}",
        diagnostic.stderr
    );
    let stderr = String::from_utf8(diagnostic.stderr).expect("UTF-8 parser diagnostic");
    assert!(
        stderr.contains("not-a-command-λ"),
        "visible argument data was not preserved: {stderr:?}"
    );
}

#[test]
fn json_clap_and_runtime_failures_are_parseable_and_nonzero() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");

    let (parse, error) = json_output(
        &mut bin(&home, &cwd).args(["--json", "not-a-command"]),
        "clap error",
    );
    assert_json_error(&parse, &error, "clap parse error");

    let missing = cwd.join("missing.jsonl");
    let (runtime, error) = json_output(
        &mut bin(&home, &cwd)
            .args(["session", "import", "--client", "omp"])
            .arg(missing)
            .arg("--json"),
        "missing session import",
    );
    assert_json_error(&runtime, &error, "session import runtime error");
}

#[test]
fn managed_mcp_requires_one_valid_explicit_scope() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");
    let invalid = [
        (
            &["--json", "mcp", "--managed"][..],
            "managed scope required",
        ),
        (
            &["--json", "mcp", "--managed", "--project", " "],
            "empty managed project",
        ),
        (
            &["--json", "mcp", "--project", "project-a"],
            "project requires managed mode",
        ),
        (
            &["--json", "mcp", "--all-projects"],
            "all-projects requires managed mode",
        ),
        (
            &[
                "--json",
                "mcp",
                "--managed",
                "--project",
                "project-a",
                "--all-projects",
            ],
            "conflicting managed scopes",
        ),
    ];
    for (args, label) in invalid {
        let (out, error) = json_output(&mut bin(&home, &cwd).args(args), label);
        assert_json_error(&out, &error, label);
    }
}

#[test]
fn search_client_filters_cover_six_clients_but_intake_remains_three_client_only() {
    let fixture = Fixture::new();
    let home = fixture.dir("home");
    let cwd = fixture.dir("workspace");
    let initialized = output(&mut bin(&home, &cwd).args(["init"]), "init");
    assert_success(&initialized, "init");
    let seed = fixture.path().join("filter-session.jsonl");
    omp_fixture(&seed, "cli-filter-seed");
    let (imported, report) = json_output(
        &mut bin(&home, &cwd)
            .args(["--json", "session", "import", "--client", "omp"])
            .arg(&seed),
        "client filter seed import",
    );
    assert_success(&imported, "client filter seed import");
    let expected_session = report["session_id"].as_str().expect("seed session id");

    for client in [
        "omp",
        "claude-code",
        "codex",
        "grok",
        "opencode",
        "cursor-agent",
    ] {
        let (out, response) = json_output(
            &mut bin(&home, &cwd).args([
                "--json",
                "search",
                "credential store",
                "--client",
                client,
            ]),
            client,
        );
        assert_success(&out, client);
        assert_eq!(response["ok"], true, "{client}: {response}");
        if client == "omp" {
            assert_eq!(response["refused"], false, "{client}: {response}");
            assert!(
                response["hits"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|hit| hit["session_id"] == expected_session),
                "known matching client hit missing: {response}"
            );
        } else {
            assert_eq!(
                response["refused"], true,
                "foreign client leaked the OMP session: {response}"
            );
            assert_eq!(
                response["hits"],
                json!([]),
                "foreign client leaked hits: {response}"
            );
        }
    }

    let source = fixture.path().join("unsupported-native-history.jsonl");
    let source_root = fixture.dir("source");
    for client in ["grok", "opencode", "cursor-agent"] {
        let (import, error) = json_output(
            &mut bin(&home, &cwd)
                .args(["--json", "session", "import", "--client", client])
                .arg(&source),
            "unsupported session import client",
        );
        assert_json_error(&import, &error, "unsupported session import client");

        let (add, error) = json_output(
            &mut bin(&home, &cwd)
                .args(["--json", "source", "add", "--client", client])
                .arg(&source_root),
            "unsupported source client",
        );
        assert_json_error(&add, &error, "unsupported source client");
    }
}
