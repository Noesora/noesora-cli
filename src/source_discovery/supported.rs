use std::env;
use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::DirExt;
use cap_std::ambient_authority;
use cap_std::fs::{Dir, ReadDir};

use super::{DiscoveryReport, SourceReport, MAX_DEPTH};

const MAX_ROOT_ERRORS: usize = 8;
const OMITTED_ERRORS: &str = "additional metadata errors omitted";
struct PlannedSource {
    report: SourceReport,
    root: Option<Dir>,
}

struct DirectoryFrame {
    dir: Dir,
    entries: ReadDir,
    depth: usize,
}
pub(super) fn discover(max_entries: u32) -> DiscoveryReport {
    let home = home_path();
    let mut sources = [
        planned_source("omp", "sessions", omp_sessions_root(&home)),
        planned_source("claude-code", "sessions", claude_sessions_root(&home)),
        planned_source("codex", "sessions", codex_root(&home, "sessions")),
        planned_source(
            "codex",
            "archived_sessions",
            codex_root(&home, "archived_sessions"),
        ),
    ];

    let mut visited_entries = 0usize;
    for index in 0..sources.len() {
        let Some(root) = sources[index].root.take() else {
            continue;
        };
        if scan_root(
            &mut sources[index].report,
            root,
            &mut visited_entries,
            max_entries,
        ) {
            for source in sources.iter_mut().skip(index + 1) {
                if source.root.take().is_some() {
                    mark_incomplete(
                        &mut source.report,
                        "global child-entry limit reached before this root was scanned",
                    );
                }
            }
            break;
        }
    }

    let complete = sources.iter().all(|source| source.report.complete);
    DiscoveryReport {
        ok: complete,
        command: "source discover",
        complete,
        max_entries,
        visited_entries,
        max_depth: MAX_DEPTH,
        sources: sources.map(|source| source.report),
        unsupported_clients: ["opencode", "cursor-agent", "grok"],
    }
}
fn home_path() -> Result<PathBuf, &'static str> {
    let home = env::var_os("HOME").filter(|value| !value.is_empty());
    let selected = match home {
        Some(value) => value,
        None => env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .ok_or("an absolute HOME or USERPROFILE is required")?,
    };
    let home = selected
        .into_string()
        .map_err(|_| "the selected home path is not representable as UTF-8")?;
    let path = PathBuf::from(home);
    if !path.is_absolute() {
        return Err("the selected home path is not absolute");
    }
    Ok(path)
}

fn omp_sessions_root(home: &Result<PathBuf, &'static str>) -> Result<PathBuf, &'static str> {
    if let Some(config_dir) = env::var_os("PI_CONFIG_DIR") {
        if !config_dir.is_empty() {
            let config_dir = config_dir
                .into_string()
                .map_err(|_| "custom OMP config roots are unsupported")?;
            if config_dir != ".omp" {
                return Err("custom OMP config roots are unsupported");
            }
        }
    }

    #[cfg(target_os = "linux")]
    if [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ]
    .into_iter()
    .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return Err("Linux XDG OMP session roots are unsupported");
    }

    let profile = active_profile()?;
    let agent_root = if let Some(profile) = profile {
        home.as_ref()
            .map_err(|error| *error)?
            .join(".omp")
            .join("profiles")
            .join(profile)
            .join("agent")
    } else if let Some(agent_dir) = absolute_override("PI_CODING_AGENT_DIR")? {
        agent_dir
    } else {
        home.as_ref()
            .map_err(|error| *error)?
            .join(".omp")
            .join("agent")
    };
    Ok(agent_root.join("sessions"))
}

fn active_profile() -> Result<Option<String>, &'static str> {
    let selected = match env::var_os("OMP_PROFILE") {
        Some(profile) => Some(profile),
        None => env::var_os("PI_PROFILE"),
    };
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected = selected
        .into_string()
        .map_err(|_| "the OMP profile is not representable as UTF-8")?;
    let normalized = selected.trim();
    if normalized.is_empty() || normalized == "default" {
        return Ok(None);
    }
    if !valid_profile_name(normalized) {
        return Err("the configured OMP profile is unsupported");
    }
    Ok(Some(normalized.to_owned()))
}

fn valid_profile_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 64
        || !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        || !bytes[1..].iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        })
        || name == "."
        || name == ".."
        || name.ends_with('.')
    {
        return false;
    }

    let basename = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    !matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(basename.len() == 4
            && (basename.starts_with("COM") || basename.starts_with("LPT"))
            && basename.as_bytes()[3].is_ascii_digit())
}

fn claude_sessions_root(home: &Result<PathBuf, &'static str>) -> Result<PathBuf, &'static str> {
    let base = match absolute_override("CLAUDE_CONFIG_DIR")? {
        Some(path) => path,
        None => home.as_ref().map_err(|error| *error)?.join(".claude"),
    };
    Ok(base.join("projects"))
}

fn codex_root(
    home: &Result<PathBuf, &'static str>,
    child: &'static str,
) -> Result<PathBuf, &'static str> {
    let base = match absolute_override("CODEX_HOME")? {
        Some(path) => path,
        None => home.as_ref().map_err(|error| *error)?.join(".codex"),
    };
    Ok(base.join(child))
}

fn absolute_override(name: &'static str) -> Result<Option<PathBuf>, &'static str> {
    let Some(value) = env::var_os(name).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| "configured root hint is not representable as UTF-8")?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("configured root hint is not absolute");
    }
    Ok(Some(path))
}

fn planned_source(
    client: &'static str,
    root_type: &'static str,
    path: Result<PathBuf, &'static str>,
) -> PlannedSource {
    let mut report = SourceReport {
        client,
        root_type,
        path: None,
        status: "unsupported_config",
        jsonl_files: 0,
        jsonl_bytes: 0,
        compressed_files: 0,
        compressed_bytes: 0,
        complete: false,
        errors: Vec::new(),
    };

    let path = match path {
        Ok(path) => path,
        Err(error) => {
            push_error(&mut report, error);
            return PlannedSource { report, root: None };
        }
    };
    if !supported_root_path(&path) {
        push_error(&mut report, "candidate root has an unsupported path format");
        return PlannedSource { report, root: None };
    }
    let Some(path_text) = path.to_str() else {
        push_error(&mut report, "candidate root is not representable as UTF-8");
        return PlannedSource { report, root: None };
    };
    report.path = Some(path_text.to_owned());
    match open_absolute_dir(&path) {
        Ok(root) => {
            report.status = "found";
            report.complete = true;
            PlannedSource {
                report,
                root: Some(root),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            report.status = "missing";
            report.complete = true;
            PlannedSource { report, root: None }
        }
        Err(error) => {
            mark_incomplete(
                &mut report,
                &format!(
                    "candidate root could not be opened safely ({:?})",
                    error.kind()
                ),
            );
            PlannedSource { report, root: None }
        }
    }
}

fn supported_root_path(path: &Path) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return false;
    }

    let raw = path.as_os_str().as_encoded_bytes();
    !(raw.starts_with(b"//") && !raw.starts_with(b"///"))
}

fn open_child_dir(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    let metadata = parent.symlink_metadata(name)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "symlink path component",
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "path component is not a directory",
        ));
    }
    parent.open_dir_nofollow(name)
}

#[cfg(unix)]
fn open_absolute_dir(path: &Path) -> io::Result<Dir> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "candidate root is not absolute",
        ));
    }
    let mut dir = Dir::open_ambient_dir(Path::new("/"), ambient_authority())?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => dir = open_child_dir(&dir, name)?,
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "candidate root has an unsupported component",
                ));
            }
        }
    }
    Ok(dir)
}

fn scan_root(
    source: &mut SourceReport,
    root: Dir,
    visited_entries: &mut usize,
    max_entries: u32,
) -> bool {
    let entries = match root.entries() {
        Ok(entries) => entries,
        Err(error) => {
            mark_incomplete(
                source,
                &format!("candidate root could not be read ({:?})", error.kind()),
            );
            return false;
        }
    };
    let mut stack = vec![DirectoryFrame {
        dir: root,
        entries,
        depth: 0,
    }];

    loop {
        let Some(depth) = stack.last().map(|frame| frame.depth) else {
            break;
        };

        if *visited_entries >= max_entries as usize {
            if has_remaining_entry(&mut stack, source) {
                mark_incomplete(source, "global child-entry limit reached");
                return true;
            }
            break;
        }

        if depth >= MAX_DEPTH {
            let next = stack
                .last_mut()
                .expect("directory frame exists")
                .entries
                .next();
            match next {
                Some(Ok(_)) => {
                    mark_incomplete(source, "maximum directory depth reached");
                    stack.pop();
                }
                Some(Err(error)) => {
                    mark_incomplete(
                        source,
                        &format!("directory entry read failed ({:?})", error.kind()),
                    );
                    stack.pop();
                }
                None => {
                    stack.pop();
                }
            }
            continue;
        }

        let next = stack
            .last_mut()
            .expect("directory frame exists")
            .entries
            .next();
        match next {
            None => {
                stack.pop();
            }
            Some(Err(error)) => {
                mark_incomplete(
                    source,
                    &format!("directory entry read failed ({:?})", error.kind()),
                );
                stack.pop();
            }
            Some(Ok(entry)) => {
                *visited_entries += 1;
                let name = entry.file_name();
                let metadata = match stack
                    .last()
                    .expect("parent directory frame exists")
                    .dir
                    .symlink_metadata(&name)
                {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        mark_incomplete(
                            source,
                            &format!("child metadata failed ({:?})", error.kind()),
                        );
                        continue;
                    }
                };

                if metadata.file_type().is_symlink() {
                    mark_incomplete(source, "symlink entry skipped");
                } else if metadata.is_dir() {
                    let child = stack
                        .last()
                        .expect("parent directory frame exists")
                        .dir
                        .open_dir_nofollow(&name);
                    match child {
                        Ok(dir) => match dir.entries() {
                            Ok(entries) => stack.push(DirectoryFrame {
                                dir,
                                entries,
                                depth: depth + 1,
                            }),
                            Err(error) => mark_incomplete(
                                source,
                                &format!("child directory could not be read ({:?})", error.kind()),
                            ),
                        },
                        Err(error) => mark_incomplete(
                            source,
                            &format!(
                                "child directory could not be opened safely ({:?})",
                                error.kind()
                            ),
                        ),
                    }
                } else if metadata.is_file() {
                    match Path::new(&name).extension() {
                        Some(extension) if extension == OsStr::new("jsonl") => {
                            add_candidate(source, false, metadata.len());
                        }
                        Some(extension) if extension == OsStr::new("zst") => {
                            add_candidate(source, true, metadata.len());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    false
}

/// Consumes iterator errors by closing only the affected directory; errors are not entries.
fn has_remaining_entry(stack: &mut Vec<DirectoryFrame>, source: &mut SourceReport) -> bool {
    loop {
        let Some(frame) = stack.last_mut() else {
            return false;
        };
        match frame.entries.next() {
            Some(Ok(_)) => return true,
            Some(Err(error)) => {
                mark_incomplete(
                    source,
                    &format!("directory entry read failed ({:?})", error.kind()),
                );
                stack.pop();
            }
            None => {
                stack.pop();
            }
        }
    }
}

fn add_candidate(source: &mut SourceReport, compressed: bool, bytes: u64) {
    if compressed {
        match source.compressed_files.checked_add(1) {
            Some(count) => source.compressed_files = count,
            None => mark_incomplete(source, "candidate file count overflowed"),
        }
        match source.compressed_bytes.checked_add(bytes) {
            Some(total) => source.compressed_bytes = total,
            None => mark_incomplete(source, "candidate byte total overflowed"),
        }
    } else {
        match source.jsonl_files.checked_add(1) {
            Some(count) => source.jsonl_files = count,
            None => mark_incomplete(source, "candidate file count overflowed"),
        }
        match source.jsonl_bytes.checked_add(bytes) {
            Some(total) => source.jsonl_bytes = total,
            None => mark_incomplete(source, "candidate byte total overflowed"),
        }
    }
}

fn mark_incomplete(source: &mut SourceReport, error: &str) {
    source.status = "incomplete";
    source.complete = false;
    push_error(source, error);
}

fn push_error(source: &mut SourceReport, error: &str) {
    if source.errors.len() < MAX_ROOT_ERRORS {
        source.errors.push(error.to_owned());
    } else if source.errors.last().map(String::as_str) != Some(OMITTED_ERRORS) {
        source.errors[MAX_ROOT_ERRORS - 1] = OMITTED_ERRORS.to_owned();
    }
}
