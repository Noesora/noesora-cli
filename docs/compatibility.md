# Compatibility and data boundaries

## Version pairing

CLI and Engine are a source pair: the CLI Cargo manifest uses the sibling Engine checkout, and `.github/engine-revision` names the required full Engine commit. Build and run a CLI only with that exact pinned Engine revision. The pin and current manifest version in a checkout are not evidence of a public release, a supported upgrade path, or a compatible older client.

The local compatibility driver passed against exact Engine baselines `fc63febea88a2d27302e77a11b4647230fae5f3b` and `f02aa81fdd3342f378746d02daed1684c2a115fc`. It checks visible rejection of unsupported canonical data, byte preservation, cited recall and recovery after unsupported blockers are removed. These synthetic local checks are not an approved production upgrade or rollback procedure. Do not run an older writer against a real store without a release-specific procedure.

## Local imports preserve their sources

Session import, neutral session ingestion, enabled-root intake, and Markdown vault import are explicit local operations. They do not modify original input files or silently promote raw session history to approved records. A malformed known native import is intentionally retained as its exact raw snapshot, returns a visible parse error, and activates zero sessions. It is not a deletion or cleanup signal.

The Engine contract states that neutral manifest v2 and correction records require compatible readers. Unsupported older readers must fail visibly, not return a partial corpus; mixed-version writers are not supported. An older reader may reject a store while v2 content remains. No downgrade or data-conversion procedure is claimed here. Preserve the source and stop if the reader reports an unsupported format; do not try an older writer against the same store.

## Store and retrieval scope

All local commands use the same user-global store (`$NOESORA_HOME/store` or `~/.noesora/store`), regardless of working directory. SQLite indexes and vectors are derived data; canonical source snapshots and record files remain authoritative. A rebuild reconstructs derived state; it is not a version downgrade or a way to make an unsupported reader compatible.

Text and semantic search are separate modes. Semantic search depends on the explicitly installed local model and indexed coverage. Model unavailability does not imply that local text retrieval is unavailable. Search returns cited evidence or a refusal; it does not generate answers. See [Troubleshooting](troubleshooting.md) for the distinct `MODEL_UNAVAILABLE` and `SEARCH_FAILED` categories.

## Platform evidence

A configured npm asset target is not a support claim. Platform availability requires that exact target's native build, package install, checksum validation, and runtime evidence. The five configured asset names are not proof that all five platforms have passed those checks. The terminal smoke harness is POSIX-only; do not infer PTY coverage on Windows from a Windows binary build or npm install.

## Evidence labels

- **SOURCE-VERIFIED:** statements above about the current command surface, package/Engine path dependency, explicit import, typed search errors, and data-format boundaries are grounded in the checked-in CLI/Engine source contracts.
- **UNRESOLVED RELEASE:** the owner has not approved a production upgrade/rollback pair. The two local fixed-baseline checks do not establish a general compatibility guarantee.
