# Noesora CLI

Public command-line client for Noesora.

This repo is the CLI only: flags, `--json`, and process exit codes. Vault logic lives in the private engine crate.

```bash
cargo run -- init
cargo run -- status
cargo run -- note --title "Retry billing" Keep credits on the workspace.
cargo run -- --json status
```
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
