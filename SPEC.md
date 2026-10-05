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
- npm publishes one `noesora` package. Install downloads the exact-version native binary from a GitHub Release and verifies its SHA-256. Targets: macOS x64/arm64, Linux x64/arm64, Windows x64.
- npm install and the launcher require Node.js 18+. Install needs network access; after install, the CLI uses the local binary and needs no Engine source or Noesora service.

## §I INTERFACES
- cmd: `noesora init [path]` → create `.noesora/vault.json`
- cmd: `noesora status` → nearest vault metadata
- cmd: `noesora note --title <title> [--status open|done|blocked] [--file <path>]... [body]` → durable record
- cmd: `noesora search <query>` → cited hits or `{ok:true,hits:[],refused:true,reason:"no_evidence",truncated:false}`; JSON/MCP include `truncated`; capped text notice ∈ stderr
- cmd: `noesora query <sql>` → read-only capped CSV
- cmd: `noesora mcp` → stdio MCP tools `search`, `query`, `note`, `handoff`; search includes `truncated`
- cmd: `noesora --json …` → one JSON object on stdout; failure `{ok:false,error}` + exit ≠ 0
- cmd: `noesora vault use <path>` → validate vault & set global default for MCP
- cmd: `noesora candidate import <path>` → copy caller-selected file bytes into nearest vault's `.noesora/candidates/`; text/JSON report destination
- file: `~/.noesora/config.json` → `{"default_vault":"<absolute path>"}`
- cmd: `noesora tui` → browse nearest vault Markdown and run cited search; read-only. Keys: `↑/↓` browse, `/` search, `Enter` run, `Esc` return from Search, `q` quit in Browse.
- npm: `npm install -g noesora` → one package downloads and verifies the matching release binary. Maintainers publish the npm package manually after its GitHub Release assets exist.

## §V INVARIANTS
V1: ∀ `--json` failure, incl clap parse → parseable `{ok:false,error}` on stdout & exit ≠ 0.
V2: ∀ search hit → engine-backed `id,type,title,path,span,hash,text`; no evidence → explicit refusal. ⊥ generated answer.
V3: Durable note/handoff writes → engine record path only; candidate import → engine candidate path only. Candidates ∉ default search; ⊥ auto-accept.
V4: `query` read-only & row-capped by engine; text result CSV.
V5: MCP stdio stdout → protocol messages only; diagnostics ∈ stderr. MCP tools reuse CLI engine results.
V6: CLI local path → 0 remote model calls; ⊥ Cloud dependency for search.
V7: ∀ MCP session → configured global vault wins over host cwd; absent/invalid config → visible error, ⊥ wrong-vault fallback.
V8: MCP `note`/`handoff` tool call → durable engine write without extra Noesora confirmation; hooks/transcripts remain candidates only.
V9: TUI resolves nearest vault from cwd, not MCP global default; absolute active vault path stays visible.
V10: TUI browsing/search writes no records, candidates, or config. Search may rebuild the derived index via engine.
V11: TUI search renders engine `SearchHit` fields or explicit `no_evidence` refusal; no generated summaries.
V12: TUI exits/errors → restore terminal raw/alternate-screen state.
V13: JSON/MCP search → `truncated=true` iff ranked hits were omitted; false on refusal. Capped text search → notice ∈ stderr; TUI → visible notice, cited hits stay browsable.
V14: ∀ engine search-result API changes → CLI, MCP, TUI compile against current Engine main at exact CLI SHA via Trusted CLI before merge.
V15: candidate import copies caller-selected source bytes unchanged to nearest vault's `.noesora/candidates/`; no record/config writes or auto-accept. Caller confirms source completeness.
V16: TUI Search mode appends every printable key, including `q`, to the query; only Browse `q` quits; `Esc` returns to Browse.
V17: supported os/arch → matching versioned GitHub Release asset; unsupported pair → explicit install error.
V18: npm launcher forwards argv, inherited stdio, and native exit code.
V19: installed npm CLI requires Node.js 18+ for its launcher; runtime needs no downloads or Engine source.
V20: package version, release tag, binary version, and checksum manifest refer to the same release.
V21: version tag → CI builds and publishes five native assets plus `SHA256SUMS`; CI never publishes to npm.
V22: npm install → downloads only the matching asset, verifies SHA-256 before install, and fails on unsupported targets, missing assets, or mismatch.
V23: `npm publish` → prepublish check requires all five assets and checksums for that package version.

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
T9|x|add manual candidate import command|V3,V15,V14,I.cmd
T10|x|fix TUI search key routing|V16,I.cmd
T11|x|add one npm package, versioned release-asset installer, and install smoke|V17,V18,V19,V20,V22,I.npm
T12|~|build five targets, verify install on each, and publish GitHub Release assets from version tags|V21,V22,T11
T13|.|publish one npm package manually after all release assets pass the prepublish check|V23,T12

## §B BUGS
id|date|cause|fix
B1|2026-10-01|`Cli::parse()` exits before JSON error handler|V1
B2|2026-10-01|parallel CLI tests reused timestamp-only temp root|atomic fixture sequence + exclusive mkdir
B3|2026-10-04|[VERIFIED] text output omitted `hit.id` despite V2 (`src/cli.rs:248`)|print ID; assert text output
B4|2026-10-03|[VERIFIED] `error[E0164]`: TUI build found old `SearchResult::Hits` match in `src/cli.rs:113`|V14
B5|2026-10-04|[VERIFIED] merge left duplicate MCP test tail; `cargo test` failed `unexpected closing delimiter` at `tests/mcp.rs:489`|remove duplicate tail
B6|2026-10-04|[VERIFIED] Trusted CLI `37205422486` E0164: tuple match on struct `SearchResult::Hits`|field match with `truncated`; V14
B7|2026-10-05|[VERIFIED] global `q` exit arm consumed Search input, closing TUI while typing|V16
