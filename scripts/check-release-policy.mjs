import { createHash } from 'node:crypto';
import { lstat, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { checkReleaseAssets, parseReleaseManifest, releaseAssetNames } from './check-release-assets.mjs';

const repository = 'Noesora/noesora-cli';
const githubApi = 'https://api.github.com';
const npmRegistry = 'https://registry.npmjs.org';
const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const packageManifest = JSON.parse(await readFile(path.join(packageRoot, 'package.json'), 'utf8'));
const expectedBranchRules = Object.freeze({
  'engine-source': new Set(['main', 'refs/pull/*/merge']),
  'npm-release': new Set(['main']),
});
const maxMetadataBytes = 1024 * 1024;
const maxAnnotatedTagDepth = 8;

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function requireRepository(env) {
  if (env.GITHUB_REPOSITORY !== repository) {
    throw new Error('GITHUB_REPOSITORY must be exactly Noesora/noesora-cli.');
  }
}

function requireSha(name, value) {
  if (typeof value !== 'string' || value.length !== 40 || !/^[0-9a-f]{40}$/.test(value)) {
    throw new Error(`${name} must be a lowercase 40-character SHA.`);
  }
  return value;
}

function isValidVersion(version) {
  if (typeof version !== 'string') return false;
  const numeric = '(?:0|[1-9]\\d*)';
  const prereleaseIdentifier = '(?:0|[1-9]\\d*|\\d*[A-Za-z-][0-9A-Za-z-]*)';
  const buildIdentifier = '[0-9A-Za-z-]+';
  const match = new RegExp(`^${numeric}\\.${numeric}\\.${numeric}(?:-${prereleaseIdentifier}(?:\\.${prereleaseIdentifier})*)?(?:\\+${buildIdentifier}(?:\\.${buildIdentifier})*)?$`).exec(version);
  return match?.[0] === version;
}

function requireReleaseVersion(env) {
  requireRepository(env);
  const version = env.RELEASE_VERSION;
  if (!isValidVersion(version) || version !== packageManifest.version) {
    throw new Error('RELEASE_VERSION must be a valid semantic version matching package.json.');
  }
  return version;
}

function requireReleaseIdentity(env) {
  const version = requireReleaseVersion(env);
  return {
    version,
    cliSha: requireSha('CLI_SHA', env.CLI_SHA),
    engineSha: requireSha('ENGINE_SHA', env.ENGINE_SHA),
    tag: `v${version}`,
  };
}

function requirePositiveInteger(name, value) {
  const match = typeof value === 'string' ? /^[1-9][0-9]*$/.exec(value) : null;
  if (!match || match[0] !== value) {
    throw new Error(`${name} must be a positive integer.`);
  }
  return value;
}

export function createRunMarker({ runId, cliSha, engineSha } = {}) {
  const id = requirePositiveInteger('GITHUB_RUN_ID', runId);
  const cli = requireSha('CLI_SHA', cliSha);
  const engine = requireSha('ENGINE_SHA', engineSha);
  return `<!-- noesora-release-run:${id};cli:${cli};engine:${engine} -->`;
}

async function readBoundedJson(response) {
  if (!response.body) throw new Error('GitHub metadata response is empty.');
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > maxMetadataBytes) {
        await reader.cancel();
        throw new Error('GitHub metadata response is too large.');
      }
      chunks.push(Buffer.from(value));
    }
  } catch (error) {
    if (error instanceof Error && error.message === 'GitHub metadata response is too large.') throw error;
    throw new Error('GitHub metadata response could not be read.');
  } finally {
    reader.releaseLock();
  }

  try {
    return JSON.parse(Buffer.concat(chunks, size).toString('utf8'));
  } catch {
    throw new Error('GitHub metadata response is not valid JSON.');
  }
}

async function githubGetJson(resourcePath, { env, fetchImpl, allow404 = false }) {
  const headers = {
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28',
  };
  if (typeof env.GH_TOKEN === 'string' && env.GH_TOKEN.length > 0) {
    headers.Authorization = `Bearer ${env.GH_TOKEN}`;
  }

  let response;
  try {
    response = await fetchImpl(`${githubApi}${resourcePath}`, {
      method: 'GET',
      headers,
      redirect: 'error',
    });
  } catch {
    throw new Error('GitHub metadata request failed.');
  }
  if (allow404 && response.status === 404) return null;
  if (!response.ok) throw new Error(`GitHub metadata request failed (HTTP ${response.status}).`);
  return readBoundedJson(response);
}

function validateRequiredReviewers(environment) {
  if (!Array.isArray(environment.protection_rules)) {
    throw new Error('Environment required-reviewer metadata is missing or unknown.');
  }
  const reviewerRules = environment.protection_rules.filter((rule) => isRecord(rule) && rule.type === 'required_reviewers');
  if (reviewerRules.length === 0) throw new Error('Environment has no required reviewers.');

  for (const rule of reviewerRules) {
    if (!Array.isArray(rule.reviewers) || rule.reviewers.length === 0) {
      throw new Error('Environment required reviewers are empty or unknown.');
    }
    for (const reviewer of rule.reviewers) {
      const idRecord = isRecord(reviewer?.reviewer) ? reviewer.reviewer : reviewer;
      if (
        !isRecord(reviewer) ||
        !['User', 'Team'].includes(reviewer.type) ||
        !isRecord(idRecord) ||
        !Number.isSafeInteger(idRecord.id) ||
        idRecord.id <= 0
      ) {
        throw new Error('Environment reviewer metadata must identify a User or Team by a positive ID.');
      }
    }
  }
}

function validateBranchPolicyResponse(environmentName, policies) {
  const expected = expectedBranchRules[environmentName];
  if (
    !isRecord(policies) ||
    !Number.isSafeInteger(policies.total_count) ||
    policies.total_count < 0 ||
    !Array.isArray(policies.branch_policies) ||
    policies.branch_policies.length !== policies.total_count
  ) {
    throw new Error('Environment branch-policy metadata is incomplete or unknown.');
  }

  const observed = new Set();
  for (const rule of policies.branch_policies) {
    if (!isRecord(rule) || rule.type !== 'branch' || typeof rule.name !== 'string' || observed.has(rule.name)) {
      throw new Error('Environment branch rules must be unique branch rules.');
    }
    observed.add(rule.name);
  }
  if (observed.size !== expected.size || [...expected].some((name) => !observed.has(name))) {
    throw new Error(`Environment ${environmentName} has an unexpected branch policy.`);
  }
}

function validateEnvironmentPolicy(environmentName, environment, policies) {
  if (!isRecord(environment) || environment.name !== environmentName) {
    throw new Error('GitHub returned unknown environment metadata.');
  }
  if (environment.can_admins_bypass !== false) {
    throw new Error('Environment admin bypass must be explicitly disabled.');
  }
  validateRequiredReviewers(environment);
  const branchPolicy = environment.deployment_branch_policy;
  if (
    !isRecord(branchPolicy) ||
    branchPolicy.custom_branch_policies !== true ||
    branchPolicy.protected_branches !== false
  ) {
    throw new Error('Environment must use explicit custom branch policies without protected-branch wildcards.');
  }
  validateBranchPolicyResponse(environmentName, policies);
}

export async function checkEnvironmentPolicy(environmentName, { env = process.env, fetchImpl = fetch } = {}) {
  requireRepository(env);
  if (!Object.hasOwn(expectedBranchRules, environmentName)) {
    throw new Error('Environment must be engine-source or npm-release.');
  }

  const environmentPath = `/repos/${repository}/environments/${encodeURIComponent(environmentName)}`;
  const environment = await githubGetJson(environmentPath, { env, fetchImpl });
  const policiesPath = `${environmentPath}/deployment-branch-policies?per_page=100`;
  const policies = await githubGetJson(policiesPath, { env, fetchImpl });
  validateEnvironmentPolicy(environmentName, environment, policies);
}

async function readPeeledTagCommit(tag, { env, fetchImpl }) {
  const refPath = `/repos/${repository}/git/ref/tags/${encodeURIComponent(tag)}`;
  const ref = await githubGetJson(refPath, { env, fetchImpl });
  if (!isRecord(ref) || !isRecord(ref.object)) throw new Error('Release tag metadata is missing.');

  let object = ref.object;
  const seen = new Set();
  for (let depth = 0; depth <= maxAnnotatedTagDepth; depth += 1) {
    if (!isRecord(object) || typeof object.type !== 'string') throw new Error('Release tag target metadata is unknown.');
    const objectSha = requireSha('Git tag object SHA', object.sha);
    if (object.type === 'commit') return objectSha;
    if (object.type !== 'tag' || seen.has(objectSha) || depth === maxAnnotatedTagDepth) {
      throw new Error('Release tag does not resolve through a bounded annotated-tag chain.');
    }
    seen.add(objectSha);

    const tagObject = await githubGetJson(`/repos/${repository}/git/tags/${objectSha}`, { env, fetchImpl });
    if (!isRecord(tagObject) || tagObject.sha !== objectSha || !isRecord(tagObject.object)) {
      throw new Error('Annotated release tag metadata is inconsistent.');
    }
    object = tagObject.object;
  }
  throw new Error('Release tag annotation chain is too deep.');
}

async function ensureReleaseIdentity(identity, { env, fetchImpl }) {
  const peeledCommit = await readPeeledTagCommit(identity.tag, { env, fetchImpl });
  if (peeledCommit !== identity.cliSha) {
    throw new Error('The live release tag does not peel to CLI_SHA.');
  }
}

export async function checkReleaseIdentity({ env = process.env, fetchImpl = fetch } = {}) {
  const identity = requireReleaseIdentity(env);
  await ensureReleaseIdentity(identity, { env, fetchImpl });
}

export async function checkRegistryVersionAbsent({ env = process.env, fetchImpl = fetch } = {}) {
  const version = requireReleaseVersion(env);
  const url = `${npmRegistry}/noesora/${encodeURIComponent(version)}`;
  let response;
  try {
    response = await fetchImpl(url, { method: 'GET', redirect: 'error' });
  } catch {
    throw new Error('npm registry preflight request failed.');
  }
  if (response.status === 404) return;
  if (response.ok) throw new Error('The requested noesora version already exists on npm.');
  throw new Error(`npm registry preflight failed (HTTP ${response.status}).`);
}

async function readRegularFile(filePath, unavailableMessage) {
  let metadata;
  try {
    metadata = await lstat(filePath);
  } catch {
    throw new Error(unavailableMessage);
  }
  if (!metadata.isFile()) throw new Error(unavailableMessage);
  try {
    return await readFile(filePath);
  } catch {
    throw new Error(unavailableMessage);
  }
}

async function readOriginalArtifacts(manifestPath) {
  if (typeof manifestPath !== 'string' || manifestPath.length === 0) {
    throw new Error('publication requires --manifest PATH for retained original CI artifacts.');
  }
  const absoluteManifestPath = path.resolve(manifestPath);
  const manifestBytes = await readRegularFile(absoluteManifestPath, 'Original retained CI manifest is unavailable.');
  const manifestText = manifestBytes.toString('utf8');
  const expectedChecksums = parseReleaseManifest(manifestText);
  const directory = path.dirname(absoluteManifestPath);

  for (const assetName of releaseAssetNames) {
    const bytes = await readRegularFile(
      path.join(directory, assetName),
      `Original retained CI asset is unavailable: ${assetName}.`,
    );
    if (bytes.length === 0) throw new Error(`Original retained CI asset is empty: ${assetName}.`);
    const actualHash = createHash('sha256').update(bytes).digest('hex');
    if (actualHash !== expectedChecksums.get(assetName)) {
      throw new Error(`Original retained CI asset does not match SHA256SUMS: ${assetName}.`);
    }
  }
  return manifestText;
}

function validateExistingRelease(release, tag, marker) {
  if (!isRecord(release) || release.tag_name !== tag) {
    throw new Error('Existing GitHub release tag does not match the requested release.');
  }
  if (release.draft !== false) throw new Error('Existing release is not a public, published release.');
  if (typeof release.body !== 'string' || !release.body.includes(marker)) {
    throw new Error('Existing release does not contain the exact current-run marker.');
  }

  const expectedNames = new Set([...releaseAssetNames, 'SHA256SUMS']);
  if (!Array.isArray(release.assets) || release.assets.length !== expectedNames.size) {
    throw new Error('Existing release must contain exactly the six expected assets.');
  }
  const observedNames = new Set();
  for (const asset of release.assets) {
    if (!isRecord(asset) || typeof asset.name !== 'string' || observedNames.has(asset.name)) {
      throw new Error('Existing release asset metadata is incomplete or duplicated.');
    }
    observedNames.add(asset.name);
  }
  if (observedNames.size !== expectedNames.size || [...expectedNames].some((name) => !observedNames.has(name))) {
    throw new Error('Existing release must contain exactly the six expected assets.');
  }
}

export async function checkPublication({ manifestPath, env = process.env, fetchImpl = fetch } = {}) {
  const manifestText = await readOriginalArtifacts(manifestPath);
  const identity = requireReleaseIdentity(env);
  const runId = requirePositiveInteger('GITHUB_RUN_ID', env.GITHUB_RUN_ID);
  const attempt = requirePositiveInteger('GITHUB_RUN_ATTEMPT', env.GITHUB_RUN_ATTEMPT);
  await ensureReleaseIdentity(identity, { env, fetchImpl });
  await checkRegistryVersionAbsent({ env, fetchImpl });

  const releasePath = `/repos/${repository}/releases/tags/${encodeURIComponent(identity.tag)}`;
  const release = await githubGetJson(releasePath, { env, fetchImpl, allow404: true });
  if (release === null) return 'create';
  if (attempt === '1') {
    throw new Error('An existing release cannot be reused by a new publish-job dispatch.');
  }

  validateExistingRelease(release, identity.tag, createRunMarker({
    runId,
    cliSha: identity.cliSha,
    engineSha: identity.engineSha,
  }));
  await checkReleaseAssets({ expectedManifest: manifestText, fetchImpl });
  return 'reuse';
}

async function runCli() {
  const [command, ...args] = process.argv.slice(2);
  if (command === 'environment') {
    if (args.length !== 1) throw new Error('Usage: node scripts/check-release-policy.mjs environment engine-source|npm-release');
    await checkEnvironmentPolicy(args[0]);
    console.error(`Verified protected environment policy for ${args[0]}.`);
    return;
  }
  if (command === 'identity') {
    if (args.length !== 0) throw new Error('Usage: node scripts/check-release-policy.mjs identity');
    await checkReleaseIdentity();
    console.error('Verified the live release tag resolves to CLI_SHA.');
    return;
  }
  if (command === 'registry') {
    if (args.length !== 0) throw new Error('Usage: node scripts/check-release-policy.mjs registry');
    await checkRegistryVersionAbsent();
    console.error('Verified the requested noesora version is absent from npm.');
    return;
  }
  if (command === 'marker') {
    if (args.length !== 0) throw new Error('Usage: node scripts/check-release-policy.mjs marker');
    requireRepository(process.env);
    process.stdout.write(`${createRunMarker({
      runId: process.env.GITHUB_RUN_ID,
      cliSha: process.env.CLI_SHA,
      engineSha: process.env.ENGINE_SHA,
    })}\n`);
    return;
  }
  if (command === 'publication') {
    if (args.length !== 2 || args[0] !== '--manifest') {
      throw new Error('Usage: node scripts/check-release-policy.mjs publication --manifest PATH');
    }
    const result = await checkPublication({ manifestPath: args[1] });
    process.stdout.write(`${result}\n`);
    return;
  }
  throw new Error('Usage: node scripts/check-release-policy.mjs environment|identity|registry|marker|publication');
}

const entry = process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href;
if (import.meta.url === entry) {
  runCli().catch((error) => {
    console.error(`noesora: release policy check failed: ${error instanceof Error ? error.message : 'unknown failure'}`);
    process.exitCode = 1;
  });
}
