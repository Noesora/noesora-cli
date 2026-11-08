# CLI release procedure and gates

## Current state

This document describes repository release controls, not a completed release. The source manifests' `0.1.0` value is not a public release decision. An owner must explicitly select the release version; the workflow then requires that version to match both manifests and the existing exact tag. No owner-approved version or publication result is established by this source tree. Do not create a guessed tag or claim binary availability or platform support from these files.

## Release identity and source gate

The manually dispatched `.github/workflows/release.yml` workflow accepts an immutable 40-character CLI commit SHA, an owner-approved version, and an explicit npm dist-tag with no default. It is restricted to dispatches from `main`, checks out that exact SHA with checkout credentials disabled, and requires the input version to match both the npm manifest and Rust package manifest. The exact tag `v<version>` must already exist and resolve to the same CLI commit; the workflow does not create a tag.

Before a release can proceed, the workflow requires successful `cli.yml` and `npm.yml` push runs on `main` for the exact CLI SHA. It also reads `.github/engine-revision`, requires a full immutable Engine commit, checks out that exact private Engine revision, and verifies the resulting commit ID. Every private Engine checkout job runs in the protected `engine-source` environment and uses the exact pin; forks cannot reach those jobs or receive the source credential. The public source-only checks are not a substitute for the paired native build gate.

The release matrix builds each configured target using the same CLI SHA and Engine pin, checks the executable's `--version` against the package version, creates the npm tarball, and exercises the packed install using the local native asset. The matrix is configuration, not proof of platform support; support requires successful public asset download, checksum and runtime evidence on each claimed platform.

## Publication order

Both GitHub environments require live, fail-closed policy checks before native/release work, and `npm-release` is checked again in the publish job after its environment approval. Configure `engine-source` with at least one required reviewer, administrator bypass disabled, custom branch policies enabled and protected-branch policies disabled. Its only allowed branch rules are `main` and `refs/pull/*/merge` (branch rules only). Configure `npm-release` with at least one required reviewer, administrator bypass disabled, custom branch policies enabled, protected-branch policies disabled, and only `main` allowed. The live checks read each environment and its complete deployment-branch-policy metadata; they require explicit `can_admins_bypass: false`, valid User/Team reviewer IDs, and exactly the allowed branch rules above. Missing, unknown, incomplete, denied or extra policy metadata stops the workflow. The owner selects reviewer identities; the source policy requires at least one reviewer and does not impose a second-reviewer rule. A manual dispatch by itself is not an approval.

The release job downloads only `noesora-release-*` artifacts from that same workflow run and regenerates `SHA256SUMS` locally from the five retained native files. These artifacts are retained for one day. The package `prepublishOnly` check receives the local manifest path and requires the remote release manifest and every remote asset's actual bytes to match those original CI hashes; remote self-consistency alone is not accepted.

Before npm publication, the workflow:

1. Checks that the current version tag peels to the exact requested `CLI_SHA` and that the npm version is absent.
2. Runs `prepublishOnly` against the versioned GitHub Release URL, bound to the regenerated local CI manifest.
3. Rechecks the npm registry and exact tag identity immediately before npm publication, then publishes to `https://registry.npmjs.org` with the explicit dist-tag and npm provenance enabled.

Before creating a public GitHub Release, the publication guard also rechecks the peeled tag identity. A new workflow dispatch refuses any existing release. Reuse is available only to a rerun of the original workflow run when the existing release has that run's exact marker and CLI/Engine identities, is public and not a draft, has exactly the six expected assets, and all remote bytes match the retained original CI manifest. This reuse is read-only: it never uploads, replaces, or deletes assets. On reruns, the build matrix does not run again; only the original publish job can use the original same-run artifacts while their one-day retention remains. Missing or expired artifacts stop recovery rather than trigger a rebuild.

The npm publication job uses OIDC trusted publishing with direct `npm publish`, not a long-lived token or staged publishing. Its runtime requires Node.js `>=22.14` and npm `>=11.5.1`; configuring trust with the `npm trust` commands requires npm `>=11.15`, account two-factor authentication and package write access. Configure the trusted publisher for `Noesora/noesora-cli`, workflow `.github/workflows/release.yml`, and environment `npm-release`, and explicitly allow direct publishing: publisher configurations created after 2026-09-03 default to stage-only. New trusted-publisher configurations must complete a successful publish within two days. There is no `NPM_TOKEN` fallback. The same-origin GitHub HTTPS/TLS checksum is the declared trust boundary; it is not an independent signature. See [npm trust](https://docs.npmjs.com/cli/commands/npm-trust/) and [trusted publishers](https://docs.npmjs.com/trusted-publishers/).

**Parent-reported, read-only preflight:** npm `whoami` returned `E401`, and `noesora@latest` returned `E404`. Package existence and ownership remain undetermined; these responses do not establish other dist-tag state or trusted-publisher configuration. The documented [`npm trust` setup](https://docs.npmjs.com/cli/commands/npm-trust/) requires the package to exist. Verify that `noesora` belongs to the intended owner before dispatch. Any initial publication needs separate explicit approval and is outside this workflow.

Staging a new package is not a private bootstrap: npm publishes a public `0.0.0-stage` placeholder. Staged publication is not used here and requires separate explicit approval; this workflow neither stages nor creates a package.

## Required operator setup and evidence

### Before dispatch: source and access

- Select and land the final Engine commit, update `.github/engine-revision` to that full SHA, and verify the exact CLI/Engine pair.
- Make the read-only `ENGINE_REPO_READ_TOKEN` available only as a secret in the `engine-source` GitHub Environment. Do not add a repository- or organization-level copy or fallback. Require a reviewer, disable administrator bypass, enable custom branch policies and disable protected-branch policies. Allow only `main` and `refs/pull/*/merge`.
- Configure `npm-release` with a required reviewer, administrator bypass disabled, custom branch policies enabled and protected-branch policies disabled. Allow only `main`. Live checks must pass before release work and again inside the publish job after approval.
- Verify that `noesora` exists under its intended owner. The account needs two-factor authentication and write access. Configure the trusted publisher for direct publication. Registry preflight results do not establish ownership or publisher readiness.
- If the package does not exist, stop for explicit approval of any initial publication. Do not use staged publishing as private setup. A new trusted-publisher configuration must complete its first successful publish within two days.

### Before dispatch: release identity

- Select the release version and set it in both `package.json` and `[package].version` in `Cargo.toml`. The checkout's version is not an approval.
- Select the exact npm dist-tag. The workflow has no default `latest` or prerelease channel.
- Protect each approved `v<version>` tag against updates and deletion before dispatch. GitHub administrators remain trusted. Source changes and the two environment approvals do not configure tag protection.
- Create the exact tag at the approved CLI SHA after merge. The release workflow verifies it but never creates it.

### After dispatch: verify results

- Record exact-SHA `cli.yml` and `npm.yml` results, the selected dist-tag and the original CI manifest. Preserve evidence that all five remote binaries match it. Record the GitHub Release assets, prepublish result, npm provenance/publication result and fresh public installs before claiming release or platform support.
- An existing npm version or registry response other than an exact `404` stops publication. Verify the registry state; do not automatically retry.

A workflow definition is not execution evidence. Do not report publication, provenance, successful installation, compatibility, platform support, or Cloud readiness until the corresponding observed evidence exists. No release workflow dispatch or provider/publication operation was performed while authoring this guide.
