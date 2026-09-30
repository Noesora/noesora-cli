use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_noesora"))
}

fn temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "noesora-cli-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&path).expect("temp");
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
    assert!(payload["error"].as_str().unwrap().contains("already exists"));
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
