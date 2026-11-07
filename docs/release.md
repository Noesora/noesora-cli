# CLI release procedure and gates

## Current state

This document describes repository release controls, not a completed release. The source manifests' `0.1.0` value is not a public release decision. An owner must explicitly select the release version; the workflow then requires that version to match both manifests and the existing exact tag. No owner-approved version or publication result is established by this source tree. Do not create a guessed tag or claim binary availability or platform support from these files.

## Release identity and source gate

The manually dispatched `.github/workflows/release.yml` workflow accepts an immutable 40-character CLI commit SHA, an owner-approved version, and an explicit npm dist-tag with no default. It is restricted to dispatches from `main`, checks out that exact SHA with checkout credentials disabled, and requires the input version to match both the npm manifest and Rust package manifest. The exact tag `v<version>` must already exist and resolve to the same CLI commit; the workflow does not create a tag.

Before a release can proceed, the workflow requires successful `cli.yml` and `npm.yml` push runs on `main` for the exact CLI SHA. It also reads `.github/engine-revision`, requires a full immutable Engine commit, checks out that exact private Engine revision, and verifies the resulting commit ID. It never builds from a moving Engine branch. Fork pull requests do not receive private Engine source or credentials; the public source-only checks are not a substitute for the paired native build gate.

The release matrix builds each configured target using the same CLI SHA and Engine pin, checks the executable's `--version` against the package version, creates the npm tarball, and exercises the packed install using the local native asset. The matrix is configuration, not proof of platform support; support requires successful public asset download, checksum and runtime evidence on each claimed platform.

## Publication order

The protected `npm-release` environment is a required workflow gate. Repository administrators must configure its required reviewer(s) and restrict deployments to `main`. A manual dispatch by itself is not an approval.

The release job requires all five matrix artifacts, writes `SHA256SUMS`, and publishes the matching GitHub Release only after confirming the existing version tag. Before npm publication it:

1. Runs the package's `prepublishOnly` check against the versioned GitHub Release URL.
2. Downloads the actual remote `SHA256SUMS` and all five remote binary assets, then computes and compares each downloaded byte stream's SHA-256 to the manifest.
3. Publishes the package to `https://registry.npmjs.org` with the explicitly supplied npm dist-tag and npm provenance enabled.

The npm publication step uses OIDC trusted publishing, not a long-lived npm token. The workflow requires a compatible npm CLI (`>=11.5.1`) and fails closed if that is unavailable or npm trusted publishing is not configured. Configure the npm package's trusted publisher for `Noesora/noesora-cli`, workflow `.github/workflows/release.yml`, and environment `npm-release`. There is no `NPM_TOKEN` fallback. The same-origin GitHub HTTPS/TLS checksum is the declared trust boundary; it is not an independent signature.

**Parent-reported, read-only preflight:** npm `whoami` returned `E401`, and `noesora@latest` returned `E404`. This does not establish package ownership, the status of other dist-tags, or npm trusted-publisher configuration. **Unresolved:** first-publish OIDC bootstrap may require an owner-authenticated npm package setup or an initial owner-authorized publication. Verify the current npm bootstrap path with the package owner before dispatch; this workflow has no token or manual-publish fallback.

## Required operator setup and evidence

Before any release dispatch, the repository owner must:

- Select a new release version; align `package.json` and `[package].version` in `Cargo.toml`. The current checkout's version must not be treated as approved.
- Select the exact npm dist-tag in the manual workflow input; the workflow has no implicit `latest`/prerelease channel choice.
- Select and land the final Engine commit, update `.github/engine-revision` to that full SHA, and verify the exact CLI/Engine pair.
- Make `ENGINE_REPO_READ_TOKEN` available to the trusted build job as a repository- or organization-level Actions secret with read-only Contents access to the private Engine repository. Effective secret availability has not been verified. Never put its value in source, output, artifacts, or support reports.
- Configure the protected GitHub Environment named `npm-release` with required human approval and branch restrictions.
- Verify the npm package owner and first-publish OIDC bootstrap path before dispatch. The reported `E401`/`E404` results do not prove that a trusted publisher is configured or that a first OIDC publish can succeed.
- Configure npm trusted publishing for the exact repository, workflow, and environment above. npm identity/configuration is not inferred from this workflow source.
- Create the exact `v<version>` tag at the approved CLI SHA after merge; the release workflow verifies but never creates it.
- Record successful exact-SHA `cli.yml` and `npm.yml` CI, the selected npm dist-tag, all five native asset/build/install checks, the GitHub Release asset list and checksums, the prepublish result, actual remote byte-hash verification, npm provenance/publication result, and fresh public installs before making any release or platform-support claim.

A workflow definition is not execution evidence. Do not report publication, provenance, successful installation, compatibility, platform support, or Cloud readiness until the corresponding observed evidence exists. No release workflow dispatch or provider/publication operation was performed while authoring this guide.
