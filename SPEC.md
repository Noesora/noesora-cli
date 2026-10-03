# SPEC

## §G GOAL
Public local binary: capture notes, retrieve cited evidence or refuse, query facts, expose same engine over stdio MCP, and browse vault in read-only terminal UI. Offline core; Cloud later.

## §C CONSTRAINTS
- Public `noesora` binary, private `../noesora-engine` library. Released binaries install without engine source; source checkout needs sibling repo.
- CLI owns clap, text/JSON output, process exits, stdio MCP. Engine owns vault, records, index, search, query. ⊥ duplicate store or model call.
- Local CLI and MCP need no account, server, remote Node, or Cloud connection.
- ⊥ universal product plugin claim. MCP-capable hosts use `noesora mcp`; other local hosts ? CLI JSON adapters.
- MCP ! bind one user-global default vault, independent of host cwd. Interactive `status`/`note` keep nearest-cwd behavior.
- MCP `note`/`handoff` tool calls write without extra Noesora per-write confirmation. Host MAY apply own approval. ⊥ silent transcript ingestion.
- Terminal UI: Ratatui + Crossterm; read-only local workspace. No graph, sync, or record editing in TUI.

## §I INTERFACES
- cmd: `noesora init [path]` → create `.noesora/vault.json`
- cmd: `noesora status` → nearest vault metadata
- cmd: `noesora note --title <title> [--status open|done|blocked] [--file <path>]... [body]` → durable record
- cmd: `noesora search <query>` → cited hits; JSON/MCP search includes `truncated`, false on refusal.
- cmd: `noesora query <sql>` → read-only capped CSV
- cmd: `noesora mcp` → stdio MCP tools `search`, `query`, `note`, `handoff`; search includes `truncated`
- cmd: `noesora --json …` → one JSON object on stdout; failure `{ok:false,error}` + exit ≠ 0
- cmd: `noesora vault use <path>` → validate vault & set global default for MCP
- file: `~/.noesora/config.json` → `{"default_vault":"<absolute path>"}`
- cmd: `noesora tui` → browse nearest vault Markdown and run cited search; read-only

## §V INVARIANTS
V1: ∀ `--json` failure, incl clap parse → parseable `{ok:false,error}` on stdout & exit ≠ 0.
V2: ∀ search hit → engine-backed `id,type,title,path,span,hash,text`; no evidence → explicit refusal. ⊥ generated answer.
V3: CLI/MCP writes → engine record path only. Candidates ∉ default search.
V4: `query` read-only & row-capped by engine; text result CSV.
V5: MCP stdio stdout → protocol messages only; diagnostics ∈ stderr. MCP tools reuse CLI engine results.
V6: CLI local path → 0 remote model calls; ⊥ Cloud dependency for search.
V7: ∀ MCP session → configured global vault wins over host cwd; absent/invalid config → visible error, ⊥ wrong-vault fallback.
V8: MCP `note`/`handoff` tool call → durable engine write without extra Noesora confirmation; hooks/transcripts remain candidates only.
V9: TUI resolves nearest vault from cwd, not MCP global default; absolute active vault path stays visible.
V10: TUI browsing/search writes no records, candidates, or config. Search may rebuild the derived index via engine.
V11: TUI search renders engine `SearchHit` fields or explicit `no_evidence` refusal; no generated summaries.
V12: TUI exits/errors → restore terminal raw/alternate-screen state.
V13: `truncated=true` → visible cap notice; cited hits remain browsable.
V14: ∀ engine search-result API changes → CLI, MCP, TUI consumers in this branch compile against sibling engine API.

## §T TASKS
id|status|task|cites
T1|x|`init`/`status`/`note` text + runtime JSON via engine|I.cmd
T2|x|clap parse errors honor `--json` failure contract|V1,I.cmd
T3|x|`search` cited hit/refusal from engine in text + JSON|V2,V6,I.cmd
T4|x|`query` read-only capped CSV + JSON through engine|V4,I.cmd
T5|x|configure one global default vault for MCP|V7,I.cmd,I.file
T6|x|stdio MCP `search`,`query`,`note`,`handoff` on same core|V3,V5,V7,V8,I.mcp
T7|x|add read-only Ratatui vault workspace with file browser and cited search|V2,V9,V10,V11,V12,I.cmd
T8|x|migrate CLI/MCP/TUI search consumers and show cap notice|V13,V14,I.cmd,I.mcp

## §B BUGS
id|date|cause|fix
B1|2026-10-01|`Cli::parse()` exits before JSON error handler|V1
B2|2026-10-01|parallel CLI tests reused timestamp-only temp root|atomic fixture sequence + exclusive mkdir
B3|2026-10-03|[VERIFIED] `error[E0164]`: TUI build found old `SearchResult::Hits` match in `src/cli.rs:113`|V14
