# Troubleshooting

## Search and model errors

Text retrieval is the default and does not require local embedding assets. If semantic search reports `MODEL_UNAVAILABLE`, inspect the model state and install the pinned assets only when you choose to allow that explicit download:

```sh
noesora model status
noesora model install
noesora index
```

`index` processes pending semantic embeddings. A model-install or indexing problem does not make text search unavailable; retry with `--mode text` to use local text retrieval:

```sh
noesora search "example question" --mode text
```

`MODEL_UNAVAILABLE` identifies unavailable model prerequisites. `SEARCH_FAILED` identifies another Engine search failure; preserve its message and report the error code rather than treating it as a no-match. In JSON mode, search errors retain their `code` and exit with status `2`:

```sh
noesora --json search "example question" --mode semantic
```

A successful response with no evidence is a refusal/no-match, not a generated answer and not a model error. A capped result may report that more ranked hits exist; raise `--limit` only up to the supported maximum of 100 if you need more cited passages.

## Sources and imports

The store is global, not selected by the current directory. Check `noesora status` and `noesora source list` before investigating an empty search. `source add --client <omp|claude-code|codex> <path>` enables only the explicit source root. Bare `noesora sync` imports configured roots once; it is not a background watcher or Cloud sync. A partial source failure is visible. In JSON mode, the response has `ok:false`, an `errors` array and a nonzero exit status even if other roots were imported.

Imports are explicit. `session import` archives one native JSONL file; `session ingest` accepts the neutral `noesora.session/v1` JSONL format; `import-vault` copies existing Markdown records without modifying the source vault. An import failure is not a successful empty import. In particular, malformed known native records intentionally leave the exact original file unchanged, retain the exact raw archive snapshot, report the parse failure, and activate zero sessions. Do not delete the source or raw snapshot as a troubleshooting step.

## npm launcher issues

The npm installer downloads the version-matched executable and checksum manifest at package-install time. A package installed with lifecycle scripts disabled (for example, `--ignore-scripts`) will not have the vendored executable; the launcher reports that condition. Reinstall the exact approved package version with lifecycle scripts enabled only after its public release and assets are available. The launcher does not fetch a replacement at runtime.

The baseline checksum trust assumption is GitHub HTTPS/TLS plus a same-origin `SHA256SUMS` manifest; this is not an independent signature. If the downloaded executable fails the digest check, do not bypass the error or substitute an unverified file. Preserve only the error code/message and a synthetic reproduction for a report.

## Minimal reports

The repository's Issues feature is enabled: [Noesora CLI Issues](https://github.com/Noesora/noesora-cli/issues). A source issue can include the CLI version, operating system/architecture as reported by the machine, command name, exact error code, and a synthetic or redacted reproducer. Do not attach transcripts, session JSONL, a vault/store, database files, model assets, environment dumps, access tokens, device codes, or private paths. Redact store paths from status output. An enabled issue tracker is not a promise of staffed support or a response time.
