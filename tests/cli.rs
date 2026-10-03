use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_noesora"))
}

fn temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("noesora-cli-{}-{nanos}-{seq}", std::process::id()));
    fs::create_dir(&path).expect("unique temp");
    path
}

fn assert_ok(out: &std::process::Output, label: &str) {
    assert!(
        out.status.success(),
        "{label} stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_and_status_json() {
    let root = temp_dir();
    let out = bin()
        .args(["init", "--json"])
        .current_dir(&root)
        .output()
        .expect("init");
    assert_ok(&out, "init");
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(payload["ok"], true);

    let nested = root.join("work");
    fs::create_dir(&nested).unwrap();
    let status = bin()
        .args(["status", "--json"])
        .current_dir(&nested)
        .output()
        .expect("status");
    assert_ok(&status, "status");
    let payload: serde_json::Value = serde_json::from_slice(&status.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["version"], 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn second_init_json_is_parseable_failure() {
    let root = temp_dir();
    let first = bin()
        .args(["init", "--json"])
        .current_dir(&root)
        .output()
        .expect("init");
    assert_ok(&first, "first init");
    let second = bin()
        .args(["init", "--json"])
        .current_dir(&root)
        .output()
        .expect("second init");
    assert_eq!(
        second.status.code(),
        Some(2),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&second.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("already exists"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn status_without_vault_json_failure() {
    let root = temp_dir();
    let status = bin()
        .args(["status", "--json"])
        .current_dir(&root)
        .output()
        .expect("status");
    assert_eq!(status.status.code(), Some(2));
    assert!(!status.stdout.is_empty());
    let payload: serde_json::Value = serde_json::from_slice(&status.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn note_writes_markdown_in_vault() {
    let root = temp_dir();
    let init = bin()
        .args(["init", "--json"])
        .current_dir(&root)
        .output()
        .expect("init");
    assert_ok(&init, "init");
    let note = bin()
        .args([
            "note",
            "--json",
            "--title",
            "Retry billing",
            "--file",
            "src/billing.rs",
            "Keep credits on the workspace.",
        ])
        .current_dir(&root)
        .output()
        .expect("note");
    assert_ok(&note, "note");
    let payload: serde_json::Value = serde_json::from_slice(&note.stdout).expect("json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "note");
    let path = PathBuf::from(payload["path"].as_str().unwrap());
    let text = fs::read_to_string(&path).expect("read note");
    assert!(text.contains("schema: noesora.record/v1"));
    assert!(text.contains("type: note"));
    assert!(text.contains("Keep credits on the workspace."));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn search_cites_local_record_and_refuses_unknown_query() {
    let root = temp_dir();
    let init = bin().arg("init").current_dir(&root).output().unwrap();
    assert_ok(&init, "init");
    let note = bin()
        .args([
            "note",
            "--title",
            "Café decision",
            "Café credits stay local.",
        ])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&note, "note");

    let found = bin()
        .args(["search", "credits", "--json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&found, "search hit");
    let payload: serde_json::Value = serde_json::from_slice(&found.stdout).unwrap();
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["truncated"], false);
    assert_eq!(payload["refused"], false);
    let hit = &payload["hits"][0];
    assert_eq!(hit["text"], "Café credits stay local.");
    let path = hit["path"].as_str().unwrap();
    let range = hit["span"].as_str().unwrap().strip_prefix('B').unwrap();
    let (start, end) = range.split_once("-B").unwrap();
    let source = fs::read_to_string(root.join(path)).unwrap();
    assert_eq!(
        source.get(start.parse::<usize>().unwrap()..end.parse::<usize>().unwrap()),
        hit["text"].as_str()
    );
    let rendered = bin()
        .args(["search", "credits"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&rendered, "search text");
    assert!(rendered.stderr.is_empty());
    let text = String::from_utf8(rendered.stdout).unwrap();
    assert!(text.contains(path));
    assert!(text.contains(&format!("ID: {}", hit["id"].as_str().unwrap())));
    assert!(text.contains(hit["span"].as_str().unwrap()));
    assert!(text.contains(hit["hash"].as_str().unwrap()));
    assert!(text.contains("Café credits stay local."));

    let missing = bin()
        .args(["search", "unicorn", "--json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&missing, "search refusal");
    let payload: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(payload["hits"], serde_json::json!([]));
    assert_eq!(payload["refused"], true);
    assert_eq!(payload["reason"], "no_evidence");
    assert_eq!(payload["truncated"], false);
    for index in 0..11 {
        let title = format!("Cap fixture {index}");
        let body = if index < 10 {
            "exactcap overcap"
        } else {
            "overcap"
        };
        let note = bin()
            .args(["note", "--title", title.as_str()])
            .arg(body)
            .current_dir(&root)
            .output()
            .unwrap();
        assert_ok(&note, "cap fixture note");
    }
    let exact = bin()
        .args(["search", "exactcap", "--json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&exact, "exact-cap search");
    let exact: serde_json::Value = serde_json::from_slice(&exact.stdout).unwrap();
    assert_eq!(exact["hits"].as_array().unwrap().len(), 10);
    assert_eq!(exact["truncated"], false);

    let capped = bin()
        .args(["search", "overcap", "--json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&capped, "over-cap search");
    let capped: serde_json::Value = serde_json::from_slice(&capped.stdout).unwrap();
    assert_eq!(capped["hits"].as_array().unwrap().len(), 10);
    assert_eq!(capped["truncated"], true);

    let capped_text = bin()
        .args(["search", "overcap"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&capped_text, "over-cap text search");
    assert_eq!(
        String::from_utf8(capped_text.stdout)
            .unwrap()
            .lines()
            .filter(|line| line.contains("records/"))
            .count(),
        10
    );
    assert!(String::from_utf8(capped_text.stderr)
        .unwrap()
        .contains("more matches exist"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn note_without_title_flag_fails_parse() {
    let root = temp_dir();
    let init = bin()
        .args(["init", "--json"])
        .current_dir(&root)
        .output()
        .expect("init");
    assert_ok(&init, "init");
    let note = bin()
        .args(["--json", "note"])
        .current_dir(&root)
        .output()
        .expect("note");
    assert_eq!(
        note.status.code(),
        Some(2),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&note.stdout),
        String::from_utf8_lossy(&note.stderr)
    );
    assert!(!note.stdout.is_empty());
    let payload: serde_json::Value = serde_json::from_slice(&note.stdout).expect("json");
    assert_eq!(payload["ok"], false);
    assert!(payload["error"].as_str().is_some_and(|e| !e.is_empty()));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn query_prints_capped_engine_rows_as_csv_and_json() {
    let root = temp_dir();
    assert_ok(
        &bin().arg("init").current_dir(&root).output().unwrap(),
        "init",
    );
    assert_ok(
        &bin()
            .args(["note", "--title", "Billing, Q4", "Keep credits here."])
            .current_dir(&root)
            .output()
            .unwrap(),
        "note",
    );
    let text = bin()
        .args(["query", "SELECT title, status FROM records"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&text, "query CSV");
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "title,status\n\"Billing, Q4\",open\n"
    );
    let json = bin()
        .args(["--json", "query", "SELECT title, status FROM records"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&json, "query JSON");
    let payload: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["command"], "query");
    assert_eq!(payload["columns"], serde_json::json!(["title", "status"]));
    assert_eq!(
        payload["rows"],
        serde_json::json!([["Billing, Q4", "open"]])
    );
    assert_eq!(payload["truncated"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn query_text_reports_truncation_outside_csv() {
    let root = temp_dir();
    assert_ok(
        &bin().arg("init").current_dir(&root).output().unwrap(),
        "init",
    );
    let sql = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<101) SELECT x FROM n";
    let out = bin()
        .args(["query", sql])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&out, "capped query");
    let csv = String::from_utf8(out.stdout).unwrap();
    assert_eq!(csv.lines().next(), Some("x"));
    assert_eq!(csv.lines().last(), Some("100"));
    assert_eq!(csv.lines().count(), 101);
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "noesora: result capped at 100 rows\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn query_write_fails_as_json_without_changing_records() {
    let root = temp_dir();
    assert_ok(
        &bin().arg("init").current_dir(&root).output().unwrap(),
        "init",
    );
    assert_ok(
        &bin()
            .args(["note", "--title", "Keep", "durable"])
            .current_dir(&root)
            .output()
            .unwrap(),
        "note",
    );
    let invalid = bin()
        .args(["--json", "query", "DELETE FROM records"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    let payload: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(payload["ok"], false);
    assert!(payload["error"].as_str().unwrap().contains("read-only"));
    let count = bin()
        .args(["query", "SELECT count(*) AS total FROM records"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_ok(&count, "count after rejected write");
    assert_eq!(String::from_utf8(count.stdout).unwrap(), "total\n1\n");
    fs::remove_dir_all(root).unwrap();
}

/// CLI with an isolated home so tests never read or write the real `~/.noesora`.
fn bin_home(home: &std::path::Path) -> Command {
    let mut cmd = bin();
    cmd.env("HOME", home).env_remove("USERPROFILE");
    cmd
}

fn init_vault(root: &std::path::Path) {
    fs::create_dir_all(root).unwrap();
    assert_ok(
        &bin().arg("init").current_dir(root).output().unwrap(),
        "init",
    );
}

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout not json ({err}): {} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn config_json(home: &std::path::Path) -> serde_json::Value {
    let text = fs::read_to_string(home.join(".noesora").join("config.json")).expect("config");
    serde_json::from_str(&text).expect("config json")
}

#[test]
fn vault_use_writes_absolute_default_and_status_keeps_nearest_cwd() {
    let base = temp_dir();
    let home = base.join("home");
    fs::create_dir(&home).unwrap();
    let default_vault = base.join("default");
    let cwd_vault = base.join("project");
    init_vault(&default_vault);
    init_vault(&cwd_vault);

    let out = bin_home(&home)
        .args(["--json", "vault", "use", "default"])
        .current_dir(&base)
        .output()
        .unwrap();
    assert_ok(&out, "vault use");
    let expected = fs::canonicalize(&default_vault).unwrap();
    let payload = json(&out);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["vault"], expected.to_str().unwrap());
    assert_eq!(
        config_json(&home),
        serde_json::json!({ "default_vault": expected.to_str().unwrap() })
    );

    // Interactive commands still resolve the nearest vault from cwd, not the default.
    let status = bin_home(&home)
        .args(["--json", "status"])
        .current_dir(&cwd_vault)
        .output()
        .unwrap();
    assert_ok(&status, "status");
    assert_eq!(
        json(&status)["vault"],
        fs::canonicalize(&cwd_vault).unwrap().to_str().unwrap()
    );
    let note = bin_home(&home)
        .args(["--json", "note", "--title", "Local", "stays in cwd vault"])
        .current_dir(&cwd_vault)
        .output()
        .unwrap();
    assert_ok(&note, "note");
    assert_eq!(
        json(&note)["vault"],
        fs::canonicalize(&cwd_vault).unwrap().to_str().unwrap()
    );
    assert!(!default_vault.join("records").exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn vault_use_replaces_previous_default_and_prints_text() {
    let base = temp_dir();
    let home = base.join("home");
    let (first, second) = (base.join("a"), base.join("b"));
    init_vault(&first);
    init_vault(&second);
    assert_ok(
        &bin_home(&home)
            .args(["vault", "use"])
            .arg(&first)
            .output()
            .unwrap(),
        "use first",
    );
    let out = bin_home(&home)
        .args(["vault", "use"])
        .arg(&second)
        .output()
        .unwrap();
    assert_ok(&out, "use second");
    let expected = fs::canonicalize(&second).unwrap();
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains(expected.to_str().unwrap()));
    assert_eq!(
        config_json(&home)["default_vault"],
        expected.to_str().unwrap()
    );
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn vault_use_rejects_invalid_targets_and_keeps_existing_config() {
    let base = temp_dir();
    let home = base.join("home");
    let good = base.join("good");
    init_vault(&good);
    assert_ok(
        &bin_home(&home)
            .args(["vault", "use"])
            .arg(&good)
            .output()
            .unwrap(),
        "use good",
    );
    let before = config_json(&home);

    let plain = base.join("plain");
    fs::create_dir(&plain).unwrap();
    let nested = good.join("src");
    fs::create_dir(&nested).unwrap();
    let corrupt = base.join("corrupt");
    fs::create_dir_all(corrupt.join(".noesora")).unwrap();
    fs::write(corrupt.join(".noesora").join("vault.json"), "{nope").unwrap();

    for (target, needle) in [
        (plain, "no vault at"),
        (nested, "no vault at"),
        (base.join("missing"), "could not access"),
        (corrupt, "not valid JSON"),
    ] {
        let out = bin_home(&home)
            .args(["--json", "vault", "use"])
            .arg(&target)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{}", target.display());
        let payload = json(&out);
        assert_eq!(payload["ok"], false);
        let error = payload["error"].as_str().unwrap();
        assert!(error.contains(needle), "{error}");
        assert_eq!(config_json(&home), before, "config changed for {error}");
    }
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn vault_use_without_home_fails_visibly() {
    let base = temp_dir();
    init_vault(&base.join("v"));
    let out = bin()
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .args(["--json", "vault", "use"])
        .arg(base.join("v"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let payload = json(&out);
    assert_eq!(payload["ok"], false);
    assert!(payload["error"].as_str().unwrap().contains("HOME"));
    fs::remove_dir_all(base).unwrap();
}
