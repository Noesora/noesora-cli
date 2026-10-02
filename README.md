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
```

With `--json`, command errors print `{ "ok": false, "error": "..." }` on stdout and exit with code 2.

Search reads local records through the engine. A hit includes its record path, byte span, whole-file hash, and quoted source text. An unknown query returns an explicit `no_evidence` refusal in JSON.

Query accepts one read-only `SELECT` or `WITH` statement. The engine refreshes its index from canonical records before reading. Text output is CSV with a header and at most 100 rows. A capped text result reports the limit on stderr; stdout stays CSV. `--json` returns `columns`, `rows` and `truncated`.

`vault use <path>` checks that `<path>` is a vault root, then saves its absolute path in `~/.noesora/config.json`. An invalid path leaves the existing config unchanged. This default is reserved for the planned `mcp` command. `status`, `note`, `search` and `query` still use the nearest vault above the working directory.

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
