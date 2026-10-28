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

### Neutral session sources

[VERIFIED SOURCE] Use `cargo run -- session ingest /path/to/neutral.jsonl` for explicit adapter-neutral intake. The file starts with one session header, followed by message rows:

```jsonl
{"type":"session","schema":"noesora.session/v1","source_namespace":"example:harbor","external_id":"chat/7","project":"/projects/harbor","title":"Credential storage"}
{"type":"message","external_id":"turn/1","role":"user","kind":"conversation","payload":{"text":"Keep credentials in the operating system credential store."},"parts":[{"field":"/text","scope":"conversation"}]}
```

[VERIFIED SOURCE] `parts` selects indexed strings through JSON pointers relative to `payload`; it does not redact stored payloads. Scope is `conversation`, `tools`, or `unclassified`. Unknown structural fields, duplicate message IDs/selectors, invalid pointers/times, and oversized metadata fail visibly. The caller must exclude secrets before intake. [Engine codec](https://github.com/Noesora/noesora-engine/blob/main/src/neutral.rs)

[VERIFIED SOURCE] Neutral results use hashed `mem:` identities and preserve the opaque external ID. `client` is null rather than a synthetic native client; absent source times display as unknown. Intake never executes archived commands or creates native resume ownership. Native-history imports remain unchanged.

```bash
cargo run -- search "credentials" --source-namespace example:harbor --project /projects/harbor
```

[RELEASE BOUNDARY] Neutral v2 manifests require coordinated compatible readers. Unsupported old readers must fail visibly rather than return a partial corpus. Do not mix old and new writers. This source change does not update an installed CLI or publish a release.

## Search and local model

[VERIFIED SOURCE] Search is local retrieval with source citations, not generated answers. Text is default; semantic retrieval is optional. Scope defaults to `conversation`. `tools` selects tool content; `all` also includes unclassified neutral parts. All projects are searched unless `--project` narrows them. `--source-namespace` is independent of `--client`. Hits include source identity, store-relative path, byte span, whole-file SHA-256, and quoted text. Session hits also include message identity, JSON pointer, and decoded-text span. Filters precede the cap and coverage. JSON reports `truncated`; no evidence is a refusal, not an answer.

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

[VERIFIED SOURCE] The standalone TUI host owns terminal initialization and restoration. The memory component draws into a supplied `Frame`/`Rect`; workers retain generation, session, and page-offset guards. Browsing stays global, including when launch project/client/namespace flags narrow search. Memory views perform no native discovery, agent launch, login, Attach action, or integration/configuration write. Native workspace work remains paused outside UM1-I2.

[VERIFIED LOCAL] The actual 72/120-column PTY flow passed global browsing, wrapped and expanded transcripts, and the archived-command canary. It selected a real search hit and checked current evidence frames against source-verified CLI/MCP IDs, path/hash, event byte span, JSON pointer, decoded-text span, and quote. The flow also checked terminal restoration before harness reset. This is synthetic local proof, not installed-harness compatibility or remote CI. [Smoke](scripts/tui-smoke.py)

## Local development

[VERIFIED SOURCE] `Cargo.toml` uses the private sibling `../noesora-engine`. `.github/engine-revision` pins UM1-I2 Engine commit `2eee8b4e5eb05716342f6728ca3a1390766c8be9`. Public CI checks formatting, script syntax, and the current-frame evidence-checker transition. It does not build or run the CLI. The private gate binds exact CLI and Engine commits on Linux/macOS.

[VERIFIED baseline] Published R1 CLI commit `75fc796958e11419b976499649f9b35f06b56f59` passed its Linux/macOS paired checks against the earlier Engine pin. That result does not attest to this typed-error change; its focused verification is recorded separately in the PR. History import support does not establish native-agent control compatibility.

[VERIFIED local] The CLI typed-error slice passed `cargo check --offline --locked --all-targets` and `cargo test --offline --locked`: 46 tests passed. Removing the typed search handler made its one collected category/recovery test fail; restoring it returned the full suite to green. An actual binary smoke under OS network denial observed both error codes, a healthy refusal, and text-search recovery. These checks do not attest to installed native-agent compatibility or exact-head paired CI for this new commit.

### UM1-I2 local verification

[VERIFIED LOCAL] `cargo test --offline --locked` passed 51 tests: 24 unit, 17 CLI, and 10 MCP. Formatter and offline locked all-target checks exited `0`. The paired Engine passed 79 tests. One obsolete fixed-scroll test was removed; incremental physical-row tests and the actual narrow/wide PTY flow cover tail reachability.

[VERIFIED LOCAL] The full cached-model smoke ran under OS network denial and printed `Neutral semantic projection and namespace-scoped citation passed`. The same run passed native late-window evidence, source integrity, and the actual current-frame TUI/CLI/MCP checks. Real renderer variants with a different selected hit, a hash corrupted only after scrolling, or a one-byte event-span error each failed the pane verifier. Restoring the exact renderer returned the full PTY flow to success.
[CORRECTION TO CI STATUS] The first paired run `40544122341` passed Linux and failed on macOS after its tests and neutral semantic search passed. macOS reported `expected: sha256:90c2f19a9fd47baa591ada6c28c2c25526c4aa2e5a4e47da4123bec9bfc36fba` and `actual: sha256:90c2f19a9fd47bsha256:90c2f19a9fd47baa591ada6c28c2c25526c4aa2e5a4e47da4123bec9bfc36fba`. This was one discrepant rendered frame. The verifier now retries that field after a redraw and still rejects a repeated wrong value. The regression passes a one-frame truncated hash and fails a stable wrong hash. The same physical PTY flow passed afterward.

[VERIFIED LOCAL] Against the pre-fix CLI commit `31819979cab3c38e78bb2666b937cd9b12cfe9df`, the new frame-transition test failed on the transient hash frame. With the fix, `python3 -m unittest discover -s tests -p 'test_tui_smoke.py' -v` reported `Ran 2 tests ... OK`; the tests cover transient recovery and stable bad-hash refusal. Public source CI runs this command.

[LIMITS] These local synthetic-fixture results preceded publication. They do not attest remote CI, a merge, an installed upgrade, or deployment. No model download or private-store change occurred. The declared Engine pin must match the exact private paired run.

Apache-2.0. See [LICENSE](LICENSE).
