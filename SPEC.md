# SPEC

## §G GOAL
Memory-only CLI/MCP/TUI over one shared Rust Engine. Neutral/native canonical archives, cited text/semantic retrieval and explicit local intake. Native execution remains paused outside UM1-I2.

## §C CONSTRAINTS
- [USER APPROVED R1] Engine owns one global archive/store and local embedding inference; CLI owns structured commands, JSON, MCP stdio and TUI.
- Global `NOESORA_HOME` / `~/.noesora` resolution only; retire nearest-vault and MCP-only vault configuration paths. No compatibility shim selecting old vaults.
- Explicit session import or explicitly enabled-source sync only. Preserve original files; no automatic historical intake or source modification.
- Conversation search default; Tools/All scope explicit; all projects default. Raw sessions distinct from approved notes/handoffs.
- [USER APPROVED UM1-I2, 2026-10-27] Supersede active R5 native workspace scope with memory-only clients and thin capture/import/access adapters. Existing native tasks remain historical paused work, not completed or discarded.
- Local memory/E5 needs no remote inference or runtime download. Model setup remains explicit. No Cloud, provider SDK/router/login or new credential store.
- Memory TUI performs no native discovery/version process, agent launch, Attach action, native execution or integration/configuration write. No bottom composer or new focus framework.

## §I INTERFACES
- env: `NOESORA_HOME` → application home; default `~/.noesora`.
- cmd: `noesora init` / `status` → global store initialize/status.
- cmd: `noesora session import --client omp|claude-code|codex <path>` → explicit archive + normalized projection; `session list` / `show` → paged session history.
- cmd: `noesora session ingest <path>` → explicit strict neutral JSONL intake through `Store::ingest_session`; no new MCP arbitrary-file ingestion tool.
- cmd: `noesora source add --client <client> <path>` / `source list` / `sync` → only explicitly enabled source roots.
- cmd: `noesora model install` / `model status` / `index` → explicit assets setup and pending-vector processing.
- cmd: `noesora search <query> --mode text|semantic [--project <project>] [--client <client>] [--source-namespace <namespace>] [--scope conversation|tools|all] [--limit 1..100]` → same source-verified evidence as TUI/MCP.
- cmd: `noesora note --title <title> [--status open|done|blocked] [--file <path>]... [body]` → global approved Markdown record; project metadata optional.
- cmd: `noesora query <sql>` → read-only capped CSV; `rebuild` → derived data recovery.
- cmd: `noesora import-vault <path>` → explicit existing-record migration without modifying originals.
- cmd: `noesora tui [--project <project>] [--client <client>] [--source-namespace <namespace>]` → standalone memory host. Session browsing stays global; launch filters affect search only. Host owns terminal RAII; component draws into supplied Frame/Rect.
- cmd: `noesora --json ...` → parseable success/error object; error exit nonzero.
- mcp: existing owner mode preserved; `mcp --managed --project <exact-id>` or `--managed --all-projects` supplies read-only search/session tools. Managed scopes disable SQL/mutation and enforce direct session-ID reads. No silent all-project default for managed connections.

## §V INVARIANTS
V1: ∀ `--json` failure, incl clap parse → parseable `{ok:false,error}` on stdout & exit ≠ 0. Search Engine failures preserve `code: MODEL_UNAVAILABLE` or `SEARCH_FAILED` and exit 2; validation and healthy refusal remain distinct.
V2: ∀ search hit → engine-backed `id,type,title,path,span,hash,text`; no evidence → explicit refusal. ⊥ generated answer.
V3: Writes use Engine global store; imported sessions remain raw sessions. No automatic source intake or promotion.
V4: `query` read-only & row-capped by engine; text result CSV.
V5: MCP stdio stdout → protocol messages only; diagnostics ∈ stderr. MCP tools reuse CLI engine results. Search Engine failures set isError:true and preserve structured ok:false/code/error; validation, policy and worker-transport failures remain distinct.
V6: CLI local path → 0 remote model calls; ⊥ Cloud dependency for search.
V7: CLI/TUI/MCP resolve same global store independent of cwd; no legacy-vault fallback.
V8: MCP note/handoff explicitly writes approved global records; session results never become approved records implicitly.
V9: TUI shows global store and all-project scope; project filter never switches database.
V10: Ordinary memory browsing/search never writes canonical source/config. R5 confirmed native-run capture and reviewed integration apply may write only their explicitly approved owned data.
V11: TUI search renders engine `SearchHit` fields or explicit `no_evidence` refusal; no generated summaries.
V12: TUI exits/errors → restore terminal raw/alternate-screen state.
V13: JSON/MCP search → `truncated=true` iff ranked hits were omitted; false on refusal. Capped text search → notice ∈ stderr; TUI → visible notice, cited hits stay browsable.
V14: CLI/MCP/TUI compile against pinned Engine revision at exact CLI SHA via private Trusted CLI before merge.
V15: Explicit session import preserves source bytes and identity; unsupported/incomplete/invalid input is a visible error, not successful empty history.
V16: TUI Search/Input appends printable text, including q. Focus-aware quit semantics follow V22; Esc steps back.

V17: Memory search/read epochs are separate from run/turn/request identities. Search cancellation cannot invalidate native output or pending decisions; expensive work stays off UI loop.
V18: Missing model, incomplete index, failed indexing and no-match states are distinguishable in TUI/JSON/MCP.
V19: Source text cannot execute terminal controls or archived commands; rendering sanitizes control characters without changing archive.
V20: Semantic inference and session retrieval work with network disabled after explicit model setup.
V21: Session list/reader page large history, show project/client/source identity and distinguish raw history from approved knowledge.
V22: q is text in prompt/query inputs. Memory-view back/close and application quit are explicit non-input actions; no text key can silently terminate a native turn.
V23: Navigation that cancels an unfinished bootstrap must reload it when returning to sessions; cancellation cannot masquerade as a loaded empty corpus or permanent loading.

V24: Native permission/clarification replies match active run, turn, request and an offered option/schema. Unanswered, unknown, stale and disconnected requests deny/cancel; no automatic approval.
V25: Managed MCP exact project authorization applies to every read, including direct IDs; arbitrary SQL/mutation tools are neither advertised nor callable.
V26: Local all-project browsing is not outbound all-project consent. Preflight shows exact client/executable/workspace/scope/capture and remote-provider disclosure. Known broader Noe MCP conflicts stop launch. No OS-sandbox or all-tool callback claim.
V27: Native resume uses only owned protocol-attested IDs; derived archive/provider IDs are never guessed or stripped.
V28: Integration apply requires exact target/scope/diff confirmation; foreign collisions, changed files and unsupported trust stop. Receipts/inverse patches contain owned data only, no full auth backup. Undo only unchanged owned entries.
V29: Terminal mark/palette reuse landing Mark.jsx/home.css; Noetic Gate stays one flat color. Tagged log rows and text labels remain readable without color alone.
V30: Capture contains user/native conversation/tool events only, not env/auth/control traffic. Immutable checkpoints and local E5 work stay off composer. Configured/connected/used are distinct claims.
V31: Native protocol/version/capabilities gate activation; ACP v1/v2 completion differs. No unattended headless fallback or new native system prompt.
V32: Transcript page range remains visible at narrow and wide widths even when source identity/path wraps beyond the metadata rows.
V33: JSON CLI mutations and MCP startup reject store-root paths that cannot be represented by their response protocol before creating canonical records.
V34: Source activation output reflects the effective enabled state returned by Engine; re-adding a disabled source cannot falsely claim that sync will import it.
V35: Plain CLI output and diagnostics render untrusted terminal controls safely while preserving ordinary Unicode, newlines and tabs; JSON keeps the original data with JSON escaping.
V36: A pending transcript result can be applied only while its requested session remains selected and its generation is current.
V37: Displayed session offsets describe the applied page; cancelled or failed requests cannot relabel retained rows with an uncommitted offset.
V38: Wrapped transcript text and expanded tool content remain fully reachable by scrolling at supported pane widths without changing source data or message identity.
V39: Public source checks cover every CLI pull request head regardless of base branch without private Engine checkout; the exact-head private paired gate remains required before merge.
V40: Explicit neutral ingest accepts arbitrary namespaces without pretending to be native clients. CLI/MCP/TUI share one Store, not per-product databases.
V41: All memory clients migrate together to namespace/external ID and optional native Client/source dates; absent times display explicitly unknown, never fabricated.
V42: Namespace search filtering is independent of native-client filtering and cannot widen managed exact-project authority. No new MCP file-ingestion tool or managed mutation.
V43: Standalone memory host retains typed-error recovery, physical-row/EOF scrolling, worker generations, readiness, paging and pre-initialization terminal RAII. No global history/focus framework or bottom composer.
V44: Memory browsing/query executes no native discovery, login, agent or archived command and changes no integration, canonical source or configuration.
V45: Neutral v2 manifests require coordinated compatible clients. Unsupported old readers fail visibly, never silently return a partial corpus. No mixed-writer support.
V46: Shared Engine/CLI/MCP/TUI preserve session/message identity, path/hash, pointer, event byte span, decoded-text span, quoted text, refusal/coverage and typed search errors. Actual PTY search selects a real hit and compares the rendered evidence pane with the source-verified Engine/CLI/MCP hit. Wrong selected identity/hash/span must fail that proof even if CLI search/transcript checks pass. Upstream locator metadata is not verified live evidence.

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

T11|x|global commands/session intake/model setup/retrieval and JSON|V1,V3,V6,V7,V15,V18,V20,I.cmd
T12|x|global MCP tools and shared evidence contracts|V2,V4,V5,V7,V8,V18,I.mcp
T13|x|session-first paged TUI with background work and explicit focus|V9,V10,V11,V12,V16,V17,V19,V21,V22,I.cmd
T14|~|behavior regressions, CI, terminal and cross-project smoke|V1,V2,V5,V7,V12,V18,V20,V21,V22,I.cmd

T15|~|first complete OMP ACP prompt/feed, interactive requests, scoped memory and run capture|V17,V24,V25,V26,V27,V29,V30,V31,I.cmd
T16|.|widen verified native paths to Grok/OpenCode/Cursor ACP and Codex/Claude control|V24,V26,V27,V31,I.cmd
T17|.|reviewed native setup/receipt/undo and verified instruction/hook templates for installed clients|V25,V26,V28,I.cmd
T18|.|preserve memory overlays, paging, controls and cancel-safe process lifecycle|V10,V12,V17,V19,V21,V22,V23,I.cmd
T19|.|union behavior/protocol/scope/conflict gates and actual native/PTY proof after separate consent|V24,V25,V26,V27,V28,V29,V30,V31,I.cmd
T20|x|neutral ingest CLI, namespace search and optional source metadata cutover|V1,V15,V40,V41,V42,V46,I.cmd
T21|x|shared MCP DTO/filter/error migration without changing managed authority|V5,V25,V41,V42,V46,I.mcp
T22|x|memory-only terminal host/component with preserved sanitization, scrolling and cancellation|V12,V19,V23,V32,V36,V37,V38,V43,V44,I.cmd
T23|~|coordinated compatibility, cross-client evidence parity, selected-hit PTY negative controls and exact-pin paired gate; exact-head remote CI remains pending|V14,V18,V20,V40,V45,V46,I.cmd,I.mcp

## §B BUGS
id|date|cause|fix
B1|2026-10-01|`Cli::parse()` exits before JSON error handler|V1
B2|2026-10-01|parallel CLI tests reused timestamp-only temp root|atomic fixture sequence + exclusive mkdir
B3|2026-10-04|[VERIFIED] text output omitted `hit.id` despite V2 (`src/cli.rs:248`)|print ID; assert text output
B4|2026-10-03|[VERIFIED] `error[E0164]`: TUI build found old `SearchResult::Hits` match in `src/cli.rs:113`|V14
B5|2026-10-04|[VERIFIED] merge left duplicate MCP test tail; `cargo test` failed `unexpected closing delimiter` at `tests/mcp.rs:489`|remove duplicate tail
B6|2026-10-04|[VERIFIED] Trusted CLI `37205422486` E0164: tuple match on struct `SearchResult::Hits`|field match with `truncated`; V14
B7|2026-10-05|[VERIFIED] global `q` exit arm consumed Search input, closing TUI while typing|V16
B8|2026-10-21|[VERIFIED static] R1 focus cutover retained Browse-only quit guard despite non-input quit contract|V22
B9|2026-10-21|[VERIFIED] R1 renderer contained literal newline/tab char constants; cargo fmt could not parse CLI|escape terminal-control character literals
B10|2026-10-21|[VERIFIED static] search invalidated bootstrap generation and Esc did not reschedule its discarded result|V23
B11|2026-10-21|[VERIFIED source] partial source sync returned exit2 with ok:true and omitted error while approved root sync route was absent|V1,V15,I.cmd
B12|2026-10-21|[VERIFIED] combined compile found non-standard String.pipe and a non-exhaustive footer focus match (artifact://962)|V14; direct formatting and complete focus labels
B13|2026-10-21|[VERIFIED] CLI/MCP fixtures compared lexical /var paths with canonical /private/var outputs on macOS (artifact://1011, artifact://1015)|canonicalize created fixture roots; preserve physical store identity assertions

B14|2026-10-21|[VERIFIED] actual PTY opened first-page-marker but two metadata rows clipped the range after long identity/snapshot text (bg_147)|V32; put range in transcript title
B15|2026-10-23|[VERIFIED source] omitted CLI/MCP record projects became empty strings, but Engine rejects empty project identities|I.cmd,I.mcp; reuse the established local default only for omitted values
B16|2026-10-23|[VERIFIED] macOS rejected the non-UTF-8 fixture directory with Illegal byte sequence before CLI execution (artifact://2123)|run the byte-path serialization fixture on Linux; retain macOS JSON failure coverage
B17|2026-10-23|[VERIFIED source] a full 50-row TUI page implied another page and allowed navigation into an empty terminal page|V21; fetch one lookahead row and ignore duplicate pending page requests
B18|2026-10-23|[VERIFIED source] cancelled bootstrap work left readiness displayed as loading while its result was invalidated|V18,V23; show not-loaded state and request startup when returning
B19|2026-10-23|[VERIFIED source] worker creation failure always populated the sessions error field even for search or transcript work|V18; route failure to the active pending operation
B20|2026-10-23|[VERIFIED source] terminal restoration guard was created after fallible initialization|V12; install restoration guard before initialization
B21|2026-10-23|[VERIFIED] cached-model CLI smoke compared lexical /var fixture paths with Engine's canonical /private/var model path (bg_25)|canonicalize the exclusive fixture root; retain exact model-path verification
B22|2026-10-23|[VERIFIED source] note creation can succeed before JSON serialization rejects a non-UTF-8 root-derived response path|V33; preflight response-safe store paths before JSON record mutation and MCP startup
B23|2026-10-23|[VERIFIED source] source add printed Enabled even when Engine preserved an existing disabled source manifest|V34; report the returned activation state without implicitly enabling it
B24|2026-10-23|[VERIFIED source] some plain-text path and error outputs bypassed the existing terminal-control renderer|V35; apply the shared rendering boundary to every plain output and diagnostic
B25|2026-10-23|[VERIFIED source] selection changes ignored pending transcript requests while transcript data was None, so an old result could remain current|V36
B26|2026-10-23|[VERIFIED source] page requests changed displayed offsets before their results applied, and cancellation retained old rows under the new offset|V37
B27|2026-10-23|[VERIFIED source] transcript visibility and scrolling counted logical lines before wrapping, leaving wrapped tails unreachable|V38
B28|2026-10-23|[VERIFIED source] the prepared public workflow filtered PR bases to main, which would omit later stacked CLI branches|V39; remove the base filter while retaining read-only permissions and no private checkout
B29|2026-10-23|[VERIFIED] the external PTY proof had a mismatched closing quote in its HTML string and failed Python compilation before CLI checks (bg_28)|repair the single quote; keep proof assertions unchanged
B30|2026-10-23|[VERIFIED] parser diagnostic fixture expected the URL inside OSC metadata to remain visible, but Clap removed that metadata before the terminal renderer (artifact://2154)|V35; test active-control denial and ordinary visible argument data instead
B31|2026-10-23|[VERIFIED source] transcript state and Paragraph scrolling clamp absolute wrapped-row offsets to u16::MAX, leaving permitted large-message tails unreachable|V38; use a bounded render window without an absolute u16 content limit
B32|2026-10-23|[VERIFIED] wrapped_row adaptation omitted an if-block delimiter and failed Rust parsing before checks (bg_32)|restore the missing brace without altering wrapping semantics
B33|2026-10-23|[VERIFIED] renderer adaptation omitted Cow and NARROW_WIDTH declarations and borrowed a temporary Span past its lifetime (artifact://2177)|restore existing declarations and bind the Span before borrowing
B34|2026-10-23|[VERIFIED] virtualized rows differed from the actual Paragraph oracle for a leading-tab line at 12 columns; source window split leadingspace one column early (artifact://2180)|V38; preserve exact source offsets and rendered grapheme widths
B35|2026-10-23|[VERIFIED] the external PTY runner used select.select without importing select and failed before TUI startup (bg_37)|restore the missing standard-library import
B36|2026-10-23|[VERIFIED] the PTY readiness predicate expected insertion/title order although Engine sorts equal-timestamp sessions by source-qualified ID (bg_39)|derive or control fixture ordering while preserving exact selected-row checks
B37|2026-10-23|[VERIFIED] a long global store path wrapped over the fixed three-row header and hid model/index readiness in the actual 120-column TUI (bg_39)|V18; bound metadata rows so readiness and query remain visible
B38|2026-10-23|[VERIFIED] real tool transcript with an unbroken 4096-character word failed with "scroll target not reached after 128 keypresses" (artifact://2235); debugger sampled the repeated suffix scan in source_grapheme_at|V38; use the pinned lazy first-grapheme iterator; actual 72/120-column diagnostic passed (artifact://2238)
B39|2026-10-27|[VERIFIED] UM1-I2 all-target check failed `E0505` at `src/memory_ui.rs:3002`: renderer regression kept borrowed IDs after moving the response into App|V46; retain only owned expected session/message IDs before moving the response; no full-result clone or new product invariant
B40|2026-10-27|[VERIFIED] palette restoration exposed `E0425` for test helper return type `Color` at `src/memory_ui.rs:3225`; the runtime no longer needs a global Color import|V43; import the existing color type only inside the test module; no new product invariant
B41|2026-10-27|[VERIFIED] selected-evidence regression collected one and failed at 72 columns although its source path was fully visible over two rows: the matcher joined vertical frame borders into the path|V46; exclude only the frame separator while retaining the exact expected source values; actual PTY proof validates the current pane separately
B42|2026-10-27|[VERIFIED] real PTY run timed out on the global page although loaded rows were rendered; cleanup then masked that error with `termios.error: (25, Inappropriate ioctl for device)`|V12,V43,V46; semantic waits inspect the current VT frame, and terminal snapshots/reset use the owned master as in the reviewed harness; raw recording remains only for control-sequence checks
B43|2026-10-27|[VERIFIED] complete client suite reported `23 passed; 2 failed`: a fixed twenty-row scroll did not reach a long Unicode tail, and a real bootstrap worker exceeded a five-second test deadline|V23,V38,V43; remove the incidental fixed-scroll test in favor of existing incremental-row and actual PTY coverage; make cancellation/current-result application deterministic without a disk-speed deadline
