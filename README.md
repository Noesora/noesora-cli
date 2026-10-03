# Noesora CLI

Public command-line client for Noesora.

This repo is the CLI only: flags, `--json`, and process exit codes. Vault logic lives in the private engine crate.

```bash
cargo run -- init
cargo run -- status
cargo run -- note --title "Retry billing" Keep credits on the workspace.
cargo run -- --json status
cargo run -- search "credits"
cargo run -- search "credits" --json
cargo run -- query 'SELECT title, status FROM records'
cargo run -- --json query 'SELECT count(*) AS total FROM records'
cargo run -- vault use ~/notes/team
cargo run -- mcp
cargo run -- tui
```

With `--json`, command errors print `{ "ok": false, "error": "..." }` on stdout and exit with code 2.

Search reads local records through the engine and returns at most 10 ranked hits. Each hit includes its record path, byte span, whole-file hash, and quoted source text. JSON and MCP responses include `truncated`, true only when more ranked hits exist. Refusals use `truncated: false`. Capped text search emits a stderr notice only when more ranked hits exist; stdout stays cited results.

Query accepts one read-only `SELECT` or `WITH` statement. The engine refreshes its index from canonical records before reading. Text output is CSV with a header and at most 100 rows. A capped text result reports the limit on stderr; stdout stays CSV. `--json` returns `columns`, `rows` and `truncated`.

`vault use <path>` validates a vault root and saves its canonical absolute path in `~/.noesora/config.json`. `status`, `note`, `search` and `query` still use the nearest vault above the working directory.

`noesora mcp` serves `search`, `query`, `note` and `handoff` over stdio, always using the configured default vault. Missing or invalid config is an error, never a cwd fallback. Tool writes create durable records without an extra Noesora confirmation; host-side approval is separate. MCP reserves stdout for protocol messages and sends diagnostics to stderr. `--json` does not wrap MCP traffic.

`cargo run -- tui` opens the nearest vault in read-only mode. Use ↑/↓ to browse Markdown files, `/` to search, Enter to run a cited search, and Esc to return. Results show engine-provided title, type, path, byte span, hash, and source text. When more than 10 hits match, the header says `More matches exist; showing the first 10.` Search may rebuild `.noesora/index.sqlite`; the TUI does not edit records, candidates, or config. `--json tui` returns a JSON error because the UI is interactive. The terminal restores on exit or an error.

Local checkout expects `../noesora-engine` next to this repo.

```text
Noesora/
  noesora-cli/      public
  noesora-engine/   private
```

```bash
cargo test
```

Apache-2.0. See [LICENSE](LICENSE).
