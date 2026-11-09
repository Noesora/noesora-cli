use serde::Serialize;

pub const DEFAULT_MAX_ENTRIES: u32 = 100_000;
pub const MAX_ENTRIES: u32 = 100_000;
const MAX_DEPTH: usize = 16;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod supported;

#[derive(Serialize)]
pub struct DiscoveryReport {
    pub ok: bool,
    pub command: &'static str,
    pub complete: bool,
    pub max_entries: u32,
    pub visited_entries: usize,
    pub max_depth: usize,
    pub sources: [SourceReport; 4],
    pub unsupported_clients: [&'static str; 3],
}

#[derive(Serialize)]
pub struct SourceReport {
    pub client: &'static str,
    pub root_type: &'static str,
    pub path: Option<String>,
    pub status: &'static str,
    pub jsonl_files: usize,
    pub jsonl_bytes: u64,
    pub compressed_files: usize,
    pub compressed_bytes: u64,
    pub complete: bool,
    pub errors: Vec<String>,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn discover(max_entries: u32) -> DiscoveryReport {
    supported::discover(max_entries)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn discover(max_entries: u32) -> DiscoveryReport {
    let platform_error = "native session metadata discovery is supported only on macOS and Linux";
    DiscoveryReport {
        ok: false,
        command: "source discover",
        complete: false,
        max_entries,
        visited_entries: 0,
        max_depth: MAX_DEPTH,
        sources: [
            unsupported_source("omp", "sessions", platform_error),
            unsupported_source("claude-code", "sessions", platform_error),
            unsupported_source("codex", "sessions", platform_error),
            unsupported_source("codex", "archived_sessions", platform_error),
        ],
        unsupported_clients: ["opencode", "cursor-agent", "grok"],
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unsupported_source(
    client: &'static str,
    root_type: &'static str,
    error: &'static str,
) -> SourceReport {
    SourceReport {
        client,
        root_type,
        path: None,
        status: "unsupported_config",
        jsonl_files: 0,
        jsonl_bytes: 0,
        compressed_files: 0,
        compressed_bytes: 0,
        complete: false,
        errors: vec![error.to_owned()],
    }
}
