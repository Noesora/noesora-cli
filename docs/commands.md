# Offline CLI commands

The commands below operate on the same user-global local store and require no Cloud account or daemon. Local search, browsing, MCP, SQL reads, and indexing do not contact Cloud. `NOESORA_HOME` changes the application home; otherwise the store is under `~/.noesora/store`. The current directory never selects another store.

## Initialize and inspect

```sh
noesora init
noesora status
noesora rebuild
```

`rebuild` reconstructs derived store data from canonical snapshots and records; it is not an import or compatibility downgrade.

## Explicit session and record intake

```sh
noesora session import --client omp /path/to/session.jsonl
noesora session import --client claude-code /path/to/session.jsonl
noesora session import --client codex /path/to/session.jsonl
noesora session ingest /path/to/neutral-session.jsonl
noesora import-vault /path/to/markdown-vault
```

Native session import and neutral ingestion preserve raw history; they do not turn conversations into approved records. Neutral ingestion accepts `noesora.session/v1` JSONL. Its `parts` selectors choose strings for indexing; they do not redact stored payloads, so remove secrets before intake. Markdown vault import leaves the original vault unchanged and does not make it the active store.

For an explicitly enabled local source root only:

```sh
noesora source add --client omp /path/to/session-root
noesora source list
noesora sync
```

`sync` is one-shot local intake of configured, enabled roots. It is not Cloud synchronization or a background watcher. No source roots are discovered automatically.

Browse archived raw sessions with paging:

```sh
noesora session list --offset 0 --limit 50
noesora session show <session-id> --offset 0 --limit 40
```

Write an explicit approved local note:

```sh
noesora note --title "Retry billing" --status open --project local --file src/billing.rs "Keep credits on the workspace."
```

## Search and model setup

```sh
noesora search "connection refused" --mode text --scope tools
noesora search "where are credentials stored" --mode semantic --project /projects/example
noesora search "credential policy" --source-namespace example:harbor --limit 20
noesora model status
noesora model install
noesora index
```

Text is the default mode. Semantic search requires an explicitly installed model and may have incomplete indexed coverage until `index` processes pending passages. Only `model install` may download the model; retrieval does not download it. Search returns cited source evidence or a refusal, not a generated answer. `--source-namespace` filters independently from `--client`; all projects are searched unless `--project` is supplied. Search limit is 1–100, default 10. Content scope defaults to `conversation`; `tools` selects tool content and `all` also includes unclassified neutral parts.

## Read-only query, TUI, and MCP

```sh
noesora query 'SELECT count(*) AS total FROM records'
noesora tui
noesora mcp
```

`query` accepts one read-only `SELECT` or `WITH` statement and returns capped rows (CSV in text mode). The TUI browses global sessions and local cited search. The default MCP owner mode exposes local search, read-only query, note/handoff writes, and raw-session reads; MCP transport is stdio.

Managed MCP must state an exact read-only scope:

```sh
noesora mcp --managed --project /projects/example
noesora mcp --managed --all-projects
```

Managed MCP exposes only search and raw-session reads for its declared scope; it is a tool policy, not an operating-system sandbox.

Use `--json` before the command, for example `noesora --json status`. Search failures preserve `MODEL_UNAVAILABLE` versus `SEARCH_FAILED`; see [Troubleshooting](troubleshooting.md). Bare `sync` remains local intake.

## Optional Cloud commands

These commands are implemented in the source checkout. Login, push and pull need a configured Cloud service; this does not establish a published or deployed service.

```sh
noesora cloud login --url <https-api-origin>
noesora cloud status
noesora cloud push --org <org-id> --record <record-id>
noesora cloud push --org <org-id> --record <record-id> --confirm <preview-sha256>
noesora cloud pull --org <org-id>
noesora cloud logout
```

The first push shows the exact selection and destination without uploading objects. Confirm with the returned digest only after reviewing the selected bytes. Each `--session <id>` discloses its whole exact raw snapshot. Known structured references are checked; prose and file metadata are not a complete citation inventory. Selected content can contain secrets.

Cloud tokens use operating-system credential storage without a plaintext fallback. A received proposal is not an approved or committed change. Revocation blocks new Cloud work; it cannot erase prior local copies.
