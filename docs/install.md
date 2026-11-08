# Install Noesora CLI

## Release status

This source tree defines a Node.js package that requires Node.js `>=18`. Its install hook downloads the version-matched native executable and `SHA256SUMS` from a GitHub Release over HTTPS, then checks the downloaded executable against that manifest. The launcher invokes the installed executable and does not download at runtime. These source declarations do not establish a published package version, available release assets, or a supported OS/architecture. Do not infer a release from the version currently in a checkout.

No public installation command is ready to run from the evidence accompanying this checkout. Once an owner-approved version has been published, use that exact version from the public npm registry:

```sh
npm install --global noesora@<approved-published-version> --registry=https://registry.npmjs.org
noesora --version
```

Replace the placeholder only with the version shown in the approved npm release. Package installation needs network access to npm and the corresponding GitHub Release. Do not use `--ignore-scripts`: it suppresses the binary-install hook and leaves the launcher without its native executable. Use the same package version as the matching `v<version>` GitHub Release.

## Contributor source build

The CLI Cargo manifest depends on a sibling `../noesora-engine` checkout. Source builds are therefore for contributors who can access both repositories and the exact Engine revision declared by the CLI pin. The Engine repository is private; this is not a general public installation route. Use your normal authorized Git access, never a credential-bearing repository URL.

Place the two repositories next to each other, then select the full commit from `.github/engine-revision` before building:

```sh
cd noesora-cli
ENGINE_SHA="$(tr -d '\r\n' < .github/engine-revision)"
git -C ../noesora-engine fetch origin "$ENGINE_SHA"
git -C ../noesora-engine checkout --detach "$ENGINE_SHA"
test "$(git -C ../noesora-engine rev-parse HEAD)" = "$ENGINE_SHA"
cargo install --locked --path .
noesora --version
```

A build is valid only for the exact Engine commit in the pin; do not substitute a branch or a newer local checkout. The matching CLI/Engine pair and Rust toolchain must be buildable on the host. This source procedure does not prove that a platform is supported or that a public binary is available.

## Local data and optional model

The CLI uses one user-global store, normally `~/.noesora/store`; set `NOESORA_HOME` to choose another application home. The current working directory does not choose a different store. Initialize and inspect it with:

```sh
noesora init
noesora status
```

Text search and local browsing do not require the embedding model. Model setup is explicit; `model install` may access the network to fetch model assets. Semantic retrieval then uses installed local assets and does not download a model at runtime:

```sh
noesora model status
noesora model install
noesora index
noesora search "example question" --mode semantic
```

See the [local command reference](commands.md) for session, source, search, record, query, TUI and MCP interfaces. See [Troubleshooting](troubleshooting.md) for typed errors and source-preserving imports. Optional Cloud commands are implemented in this checkout and need a configured Cloud service. They are not required for local use. This source checkout is not a published release.
