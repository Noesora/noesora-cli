# Noesora CLI

Public command-line client for Noesora. The CLI, MCP stdio server, and terminal browser share one local, user-global store; the current working directory never selects a vault or database.

## Store and commands

By default, the application home is `~/.noesora` and the store is `~/.noesora/store`. Set `NOESORA_HOME` to change the application home; the store is then `$NOESORA_HOME/store`.

```bash
cargo run -- init
cargo run -- status
cargo run -- search "credits" --mode text
cargo run -- query 'SELECT count(*) AS total FROM records'
cargo run -- note --title "Retry billing" Keep credits on the workspace.
cargo run -- tui
cargo run -- mcp
```

`--json` is a global CLI option, for example `cargo run -- --json status`. Successful command JSON payloads include `"ok": true`; fields vary by command. Non-MCP CLI errors print `{"ok":false,"error":"..."}` to stdout and exit with code 2; text-mode errors go to stderr. A partial `sync --json` keeps successful imports in its `imports` array, returns source failures in `errors` with `"ok": false`, and exits with code 2. JSON CLI mutations and MCP startup preflight store-root paths for safe response encoding before creating canonical records. Plain-text output and diagnostics sanitize untrusted terminal controls while preserving ordinary Unicode, newlines, and tabs; JSON preserves original data with JSON escaping. MCP startup diagnostics go to stderr, and `--json` does not wrap MCP protocol traffic on stdout.

## Import and migration

Raw session history has three explicit JSONL adapters: OMP, Claude Code, and Codex. Import one file with its matching client:

```bash
cargo run -- session import --client omp /path/to/session.jsonl
cargo run -- session import --client claude-code /path/to/session.jsonl
cargo run -- session import --client codex /path/to/session.jsonl
```

Imports preserve the source file and store raw session history. They do not promote conversations into approved records. For explicitly enabled roots, add a source, inspect configured roots, and run a one-shot sync:

```bash
cargo run -- source add --client omp /path/to/session-root
cargo run -- source list
cargo run -- sync
```

No roots are discovered from the working directory, and sync is not a background watcher. To bring in existing Markdown records, use `cargo run -- import-vault /path/to/old-vault`; it imports records without modifying the originals. This does not select that vault as the CLI store. Existing vaults and session logs are not migrated implicitly.

## Search and local model

Search is local retrieval with source citations, not generated answers. Text is the default mode; semantic retrieval is optional. Scope defaults to `conversation`; use `--scope tools` to search tool messages or `--scope all` to include both. All projects are searched unless `--project` narrows the search. Each hit carries its source ID, type, title, project/client/session metadata, store-relative path, byte span, whole-file SHA-256, and quoted source text. The default limit is 10; `--limit` accepts 1 to 100. JSON reports `truncated`; a no-evidence result is an explicit refusal rather than an answer.

```bash
cargo run -- search "connection refused" --mode text --scope tools
cargo run -- model status
cargo run -- model install
cargo run -- index
cargo run -- search "where are credentials stored" --mode semantic
```

`model install` installs the pinned local embedding model; `index` builds pending embeddings. Semantic search reports whether index coverage is complete. Text search and browsing remain available if the model is unavailable.

Search Engine failures in `--json` mode return `{"ok":false,"code":"MODEL_UNAVAILABLE","error":"..."}` or `{"ok":false,"code":"SEARCH_FAILED","error":"..."}` and exit with code 2. The first category identifies unavailable semantic-model prerequisites; the second covers other search failures. A healthy no-match refusal remains a successful response, and generic validation errors do not receive an Engine search category.

`query` accepts one read-only `SELECT` or `WITH` statement. Text output is CSV with a header and at most 100 rows; JSON returns `columns`, `rows`, and `truncated`. Capped text output reports the cap on stderr while stdout stays CSV.

The smoke driver accepts `node scripts/smoke.mjs [<binary>] [--model | --model-dir <installed-directory>] [--keep]`. The default binary is `target/debug/noesora`. `--model` installs the model for the isolated smoke store and may download assets; `--model-dir` instead supplies a previously installed pinned cache and validates its manifest/model identity. These options are mutually exclusive. `--keep` retains the isolated smoke store. With `--model-dir`, model status, indexing, and semantic retrieval run in offline mode. On macOS those checks use a network-denying `sandbox-exec` policy; other platforms do not get that OS-level network boundary.

## MCP access

`cargo run -- mcp` starts the owner connection against the global store. It exposes `search`, read-only `query`, durable `note` and `handoff` writes, and raw-history `session_list` and `session_read`. MCP record writes do not ask Noesora for a second confirmation; approval by the host application is separate. An omitted project on an owner note or handoff defaults to `local`; an explicitly empty project is rejected.

Managed MCP requires exactly one explicit scope:

```bash
cargo run -- mcp --managed --project /projects/harbor
cargo run -- mcp --managed --all-projects
```

`--project` must be an exact, nonempty persisted project identity. Project mode exposes only that project. `--all-projects` exposes all projects read-only. Both managed modes advertise only `search`, `session_list`, and `session_read`; `query`, `note`, and `handoff` are omitted from tool discovery and are also rejected by direct method guards. Managed access is an MCP tool policy, not a native operating-system sandbox.

MCP search Engine failures set `isError: true` and preserve `ok: false`, `code`, and `error` in structured content. The codes are `MODEL_UNAVAILABLE` and `SEARCH_FAILED`. Healthy refusals remain successful tool results; argument, policy and worker-transport errors are not mislabeled as model failures.

[VERIFIED local] The MCP typed-error slice passed the offline all-target check and full 47-test suite. Removing only the structured `code` field made its one collected stdio regression fail. An actual CLI/MCP wire smoke under OS network denial observed both categories, `isError: true`, healthy refusals, recovery and independently verified citations; all children exited normally and were reaped. Exact-head private paired CI for this new commit remains pending.

## Terminal browser

`cargo run -- tui` opens the global session browser and local cited search. The session list fetches 50 entries plus one lookahead to detect another page; transcripts page 20 messages at a time. Use ↑/↓ to move through sessions or scroll details, Enter to open a session or run a search, Tab to change focus, and PageUp/PageDown to page sessions or transcripts. In a transcript, ←/→ selects messages and `x` expands or collapses a tool message. Press `r` in the session list to refresh.

Press `/` to search. Type the query (including `q`), Backspace to edit, Enter to run it, and `m` to switch between text and semantic search. In non-input views, `q` quits; in the search input, `q` is query text. Ctrl-C quits. Esc backs out of a detail/search view, and Esc from the session list quits.

The TUI loads session pages, store/index readiness, and model status in background workers. Loading, incomplete indexing, unavailable model, and operation errors are displayed in the interface. Browsing and search do not import or sync history, edit sessions or approved records, or change source configuration; derived index state remains Engine-owned. The terminal restoration guard runs on normal exit and errors. This R1 UI is a read-only memory browser, not a native-agent workspace. The R02 UI is approved but not implemented; native-provider and native-session compatibility are not verified. Fresh macOS PTY proof passed paging, transcript visibility, races, and restoration. It observed `q` as query text in search input and as a quit from session-list focus; other non-input focus paths were not tested. Page/transcript worker completion and partial-initialization entry/terminal-attribute transition while the child was alive were unobserved.

## Local development

`Cargo.toml` uses the private sibling `../noesora-engine`; `.github/engine-revision` pins `fc63febea88a2d27302e77a11b4647230fae5f3b`. Public CI checks source formatting and smoke-driver syntax, not CLI behavior. The private Trusted CLI workflow verifies the exact CLI SHA and declared Engine pin before merge.

[VERIFIED baseline] Published R1 CLI commit `75fc796958e11419b976499649f9b35f06b56f59` passed its Linux/macOS paired checks against the earlier Engine pin. That result does not attest to this typed-error change; its focused verification is recorded separately in the PR. History import support does not establish native-agent control compatibility.

[VERIFIED local] The CLI typed-error slice passed `cargo check --offline --locked --all-targets` and `cargo test --offline --locked`: 46 tests passed. Removing the typed search handler made its one collected category/recovery test fail; restoring it returned the full suite to green. An actual binary smoke under OS network denial observed both error codes, a healthy refusal, and text-search recovery. These checks do not attest to installed native-agent compatibility or exact-head paired CI for this new commit.

Apache-2.0. See [LICENSE](LICENSE).
