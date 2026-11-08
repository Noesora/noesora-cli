import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { test } from 'node:test';
import { copyFile, mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import { checkReleaseAssets, releaseAssetNames as assetNames } from '../scripts/check-release-assets.mjs';

import {
  checkEnvironmentPolicy,
  checkPublication,
  checkRegistryVersionAbsent,
  checkReleaseIdentity,
  createRunMarker,
} from '../scripts/check-release-policy.mjs';
import { installBinary } from '../scripts/install-binary.mjs';

const packageManifest = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'));
const repository = 'Noesora/noesora-cli';
const cliSha = 'a'.repeat(40);
const engineSha = 'b'.repeat(40);



function manifestFor(bytes) {
  const digest = createHash('sha256').update(bytes).digest('hex');
  return assetNames.map((name) => `${digest}  ${name}`).join('\n') + '\n';
}

function releaseEnv(overrides = {}) {
  return {
    GITHUB_REPOSITORY: repository,
    CLI_SHA: cliSha,
    ENGINE_SHA: engineSha,
    RELEASE_VERSION: packageManifest.version,
    GITHUB_RUN_ID: '123456789',
    GITHUB_RUN_ATTEMPT: '2',
    ...overrides,
  };
}

function jsonResponse(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function textResponse(value, status = 200) {
  return new Response(value, { status, headers: { 'content-type': 'text/plain' } });
}

function routeFetch(routes, requests = []) {
  return async (input, init = {}) => {
    const url = String(input);
    requests.push({
      url,
      method: init.method ?? 'GET',
      headers: new Headers(init.headers),
    });
    const handler = routes.get(url);
    if (!handler) throw new Error('Unexpected request in release-policy fixture.');
    return typeof handler === 'function' ? handler() : handler;
  };
}

function manifestForAssets(assets) {
  return assetNames.map((name) => {
    const digest = createHash('sha256').update(assets.get(name)).digest('hex');
    return `${digest}  ${name}`;
  }).join('\n') + '\n';
}

function healthyReleaseTransport(assets, requests) {
  const manifest = manifestForAssets(assets);
  return {
    manifest,
    fetchImpl: async (input) => {
      const url = new URL(String(input));
      requests.push(url.href);
      const assetName = path.basename(url.pathname);
      if (assetName === 'SHA256SUMS') return textResponse(manifest);
      assert(assets.has(assetName));
      return new Response(assets.get(assetName));
    },
  };
}

async function assertRejectedBaseUrlBeforeFetching(baseUrl) {
  const requests = [];
  const { manifest, fetchImpl } = healthyReleaseTransport(originalAssets(), requests);
  await assert.rejects(checkReleaseAssets({ baseUrl, expectedManifest: manifest, fetchImpl }));
  assert.deepEqual(requests, []);
}

function originalAssets() {
  return new Map(assetNames.map((name) => [name, Buffer.from(`retained CI artifact: ${name}`)]));
}

async function withOriginalArtifacts(callback, assets = originalAssets()) {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'noesora-release-policy-'));
  try {
    const manifestPath = path.join(directory, 'SHA256SUMS');
    const manifest = manifestForAssets(assets);
    await writeFile(manifestPath, manifest);
    for (const [name, bytes] of assets) await writeFile(path.join(directory, name), bytes);
    return await callback({ directory, manifestPath, manifest, assets });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

function validEnvironment(name) {
  return {
    name,
    can_admins_bypass: false,
    protection_rules: [
      {
        type: 'required_reviewers',
        reviewers: [{ type: 'User', reviewer: { id: 12345 } }],
      },
      { type: 'branch_policy' },
    ],
    deployment_branch_policy: {
      protected_branches: false,
      custom_branch_policies: true,
    },
  };
}

function validBranchPolicies(name) {
  const names = name === 'engine-source' ? ['main', 'refs/pull/*/merge'] : ['main'];
  return {
    total_count: names.length,
    branch_policies: names.map((branch) => ({ name: branch, type: 'branch' })),
  };
}

function environmentFetch(name, {
  environment = validEnvironment(name),
  policies = validBranchPolicies(name),
  environmentStatus = 200,
  policiesStatus = 200,
  requests = [],
} = {}) {
  const prefix = `https://api.github.com/repos/${repository}/environments/${name}`;
  return routeFetch(new Map([
    [`${prefix}`, () => jsonResponse(environment, environmentStatus)],
    [`${prefix}/deployment-branch-policies?per_page=100`, () => jsonResponse(policies, policiesStatus)],
  ]), requests);
}

function identityUrls(env = releaseEnv()) {
  const prefix = `https://api.github.com/repos/${repository}`;
  return {
    ref: `${prefix}/git/ref/tags/v${env.RELEASE_VERSION}`,
    tag: `${prefix}/git/tags/${'c'.repeat(40)}`,
  };
}

function expectedMarker(env = releaseEnv()) {
  return createRunMarker({
    runId: env.GITHUB_RUN_ID,
    cliSha: env.CLI_SHA,
    engineSha: env.ENGINE_SHA,
  });
}

function releaseAssetMetadata() {
  return [...assetNames, 'SHA256SUMS'].map((name) => ({ name }));
}

function existingRelease(env = releaseEnv(), overrides = {}) {
  return {
    tag_name: `v${env.RELEASE_VERSION}`,
    draft: false,
    body: `${expectedMarker(env)}\n`,
    assets: releaseAssetMetadata(),
    ...overrides,
  };
}

function publicationFetch({ env = releaseEnv(), manifest, assets, releaseStatus = 404, registryStatus = 404, release, requests = [] }) {
  const { ref } = identityUrls(env);
  const apiPrefix = `https://api.github.com/repos/${repository}`;
  const registryUrl = `https://registry.npmjs.org/noesora/${encodeURIComponent(env.RELEASE_VERSION)}`;
  const releaseUrl = `${apiPrefix}/releases/tags/v${env.RELEASE_VERSION}`;
  const assetBase = `https://github.com/${repository}/releases/download/v${env.RELEASE_VERSION}`;
  const routes = new Map([
    [ref, () => jsonResponse({ object: { type: 'commit', sha: env.CLI_SHA } })],
    [registryUrl, () => textResponse('', registryStatus)],
    [releaseUrl, () => jsonResponse(release ?? {}, releaseStatus)],
    [`${assetBase}/SHA256SUMS`, () => textResponse(manifest)],
  ]);
  for (const name of assetNames) routes.set(`${assetBase}/${name}`, () => new Response(assets.get(name)));
  return routeFetch(routes, requests);
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function waitForReadyChild(child) {
  return new Promise((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => finish(new Error('Copied Node child did not become ready.')), 20000);
    const finish = (error) => {
      clearTimeout(timer);
      child.stdout.off('data', onData);
      child.off('error', onError);
      child.off('exit', onExit);
      if (error) reject(error);
      else resolve();
    };
    const onData = (chunk) => {
      output += chunk;
      if (output.includes('ready')) finish();
    };
    const onError = (error) => finish(error);
    const onExit = (code) => finish(new Error(`Copied Node child exited before ready (${code}).`));
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', onData);
    child.once('error', onError);
    child.once('exit', onExit);
  });
}

function waitForChildExit(child, timeoutMs) {
  return new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) {
      resolve(true);
      return;
    }
    let timer;
    const finish = (stopped) => {
      clearTimeout(timer);
      child.off('exit', onExit);
      resolve(stopped);
    };
    const onExit = () => finish(true);
    child.once('exit', onExit);
    timer = setTimeout(() => finish(false), timeoutMs);
  });
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill();
  if (await waitForChildExit(child, 5000)) return;
  child.kill();
  if (!await waitForChildExit(child, 2000)) {
    throw new Error('Copied Node child could not be stopped within the cleanup deadline.');
  }
}

test('V61 publication rejects remote bytes that differ from retained CI artifacts', async () => {
  const originalCiBytes = Buffer.from('original synthetic CI artifact');
  const differentRemoteBytes = Buffer.from('different synthetic remote artifact');
  const server = createServer((request, response) => {
    const name = request.url.slice(1);
    if (name === 'SHA256SUMS') {
      response.writeHead(200).end(manifestFor(differentRemoteBytes));
    } else if (assetNames.includes(name)) {
      response.writeHead(200).end(differentRemoteBytes);
    } else {
      response.writeHead(404).end();
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  try {
    const { port } = server.address();
    await assert.rejects(checkReleaseAssets({
      baseUrl: `http://127.0.0.1:${port}`,
      expectedManifest: manifestFor(originalCiBytes),
    }));
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test('V60 accepts both exact protected-environment branch policies', async () => {
  for (const name of ['engine-source', 'npm-release']) {
    const requests = [];
    const env = name === 'engine-source' ? releaseEnv({ GH_TOKEN: 'fixture-token-only' }) : releaseEnv();
    await checkEnvironmentPolicy(name, { env, fetchImpl: environmentFetch(name, { requests }) });
    assert.equal(requests.length, 2);
    assert(requests.every((request) => request.method === 'GET'));
    assert.equal(requests[0].headers.get('authorization'), name === 'engine-source' ? 'Bearer fixture-token-only' : null);
    assert.match(requests[1].url, /deployment-branch-policies\?per_page=100$/);
  }
});

test('V60 fails closed for missing or unsafe environment policy metadata', async () => {
  const noBypassValue = validEnvironment('engine-source');
  delete noBypassValue.can_admins_bypass;
  const noReviewers = validEnvironment('engine-source');
  noReviewers.protection_rules[0].reviewers = [];
  const invalidReviewer = validEnvironment('engine-source');
  invalidReviewer.protection_rules[0].reviewers = [{ type: 'Organization', reviewer: { id: 12345 } }];
  const bypassEnabled = validEnvironment('engine-source');
  bypassEnabled.can_admins_bypass = true;
  const protectedBranchFallback = validEnvironment('engine-source');
  protectedBranchFallback.deployment_branch_policy.protected_branches = true;
  const customPoliciesDisabled = validEnvironment('engine-source');
  customPoliciesDisabled.deployment_branch_policy.custom_branch_policies = false;
  const branchWildcard = validBranchPolicies('engine-source');
  branchWildcard.branch_policies.push({ name: '*', type: 'branch' });
  branchWildcard.total_count += 1;
  const branchTag = validBranchPolicies('engine-source');
  branchTag.branch_policies[0].type = 'tag';
  for (const fixture of [
    { environment: noBypassValue },
    { environment: noReviewers },
    { environment: invalidReviewer },
    { environment: bypassEnabled },
    { policies: branchWildcard },
    { policies: branchTag },
    { policies: { branch_policies: validBranchPolicies('engine-source').branch_policies } },
    { environment: protectedBranchFallback },
    { environment: customPoliciesDisabled },
  ]) {
    await assert.rejects(checkEnvironmentPolicy('engine-source', {
      env: releaseEnv(),
      fetchImpl: environmentFetch('engine-source', fixture),
    }));
  }

  await assert.rejects(checkEnvironmentPolicy('engine-source', {
    env: releaseEnv(),
    fetchImpl: environmentFetch('engine-source', { environmentStatus: 403 }),
  }), (error) => error.message === 'GitHub metadata request failed (HTTP 403).');
  await assert.rejects(checkEnvironmentPolicy('engine-source', {
    env: releaseEnv(),
    fetchImpl: environmentFetch('engine-source', { policiesStatus: 503 }),
  }), (error) => error.message === 'GitHub metadata request failed (HTTP 503).');
  await assert.rejects(checkEnvironmentPolicy('engine-source', {
    env: releaseEnv({ GITHUB_REPOSITORY: 'untrusted/fork' }),
    fetchImpl: environmentFetch('engine-source'),
  }), /exactly Noesora\/noesora-cli/);
});

test('V60 refuses a truncated branch-policy page despite valid visible rules', async () => {
  const policies = validBranchPolicies('engine-source');
  policies.total_count += 1;
  await assert.rejects(checkEnvironmentPolicy('engine-source', {
    env: releaseEnv(),
    fetchImpl: environmentFetch('engine-source', { policies }),
  }));
});

test('V61 identity peels annotated tags and compares the exact CLI commit', async () => {
  const env = releaseEnv();
  const tagSha = 'c'.repeat(40);
  const urls = identityUrls(env);
  const requests = [];
  const fetchImpl = routeFetch(new Map([
    [urls.ref, () => jsonResponse({ object: { type: 'tag', sha: tagSha } })],
    [urls.tag, () => jsonResponse({ sha: tagSha, object: { type: 'commit', sha: env.CLI_SHA } })],
  ]), requests);
  await checkReleaseIdentity({ env, fetchImpl });
  assert.equal(requests.length, 2);
  assert(requests.every((request) => request.method === 'GET'));
  assert.equal(requests[0].headers.get('authorization'), null);

  const wrongCommit = 'd'.repeat(40);
  await assert.rejects(checkReleaseIdentity({
    env,
    fetchImpl: routeFetch(new Map([
      [urls.ref, () => jsonResponse({ object: { type: 'commit', sha: wrongCommit } })],
    ])),
  }), /does not peel to CLI_SHA/);
  const wrongAnnotatedTag = 'd'.repeat(40);
  await assert.rejects(checkReleaseIdentity({
    env,
    fetchImpl: routeFetch(new Map([
      [urls.ref, () => jsonResponse({ object: { type: 'tag', sha: tagSha } })],
      [urls.tag, () => jsonResponse({ sha: tagSha, object: { type: 'commit', sha: wrongAnnotatedTag } })],
    ])),
  }), /does not peel to CLI_SHA/);
  await assert.rejects(checkReleaseIdentity({
    env: releaseEnv({ CLI_SHA: 'A'.repeat(40) }),
    fetchImpl,
  }), /lowercase 40-character/);
});

test('V62 npm registry absence requires an exact 404 and performs only a public GET', async () => {
  const env = releaseEnv();
  const url = `https://registry.npmjs.org/noesora/${encodeURIComponent(env.RELEASE_VERSION)}`;
  const requests = [];
  await checkRegistryVersionAbsent({
    env,
    fetchImpl: routeFetch(new Map([[url, () => textResponse('not found', 404)]]), requests),
  });
  assert.deepEqual(requests.map(({ method }) => method), ['GET']);
  assert.equal(requests[0].headers.get('authorization'), null);

  await assert.rejects(checkRegistryVersionAbsent({
    env,
    fetchImpl: routeFetch(new Map([[url, () => textResponse('published', 200)]])),
  }), /already exists on npm/);
  const denied = textResponse('untrusted registry body', 403);
  await assert.rejects(checkRegistryVersionAbsent({
    env,
    fetchImpl: routeFetch(new Map([[url, () => denied]])),
  }), (error) => error.message === 'npm registry preflight failed (HTTP 403).');
  assert.equal(denied.bodyUsed, false);
});

test('release marker is exact and validates the run identity', () => {
  const env = releaseEnv();
  assert.equal(
    expectedMarker(env),
    `<!-- noesora-release-run:${env.GITHUB_RUN_ID};cli:${cliSha};engine:${engineSha} -->`,
  );
  assert.throws(() => createRunMarker({ runId: '0', cliSha, engineSha }), /positive integer/);
  assert.throws(() => createRunMarker({ runId: env.GITHUB_RUN_ID, cliSha: 'F'.repeat(40), engineSha }), /lowercase 40-character/);
});

test('V61 expected manifest accepts matching downloaded remote assets', async () => {
  const bytes = Buffer.from('retained original artifact');
  const manifest = manifestFor(bytes);
  const requests = [];
  const baseUrl = 'http://127.0.0.1:43129/release';
  await checkReleaseAssets({
    baseUrl,
    expectedManifest: manifest,
    fetchImpl: async (url) => {
      requests.push(String(url));
      return String(url).endsWith('/SHA256SUMS') ? textResponse(manifest) : new Response(bytes);
    },
  });
  assert.equal(requests.length, 6);
  assert(requests.every((url) => url.startsWith(baseUrl)));
});

test('release asset checks reject non-loopback base URL overrides before fetching', async () => {
  await assertRejectedBaseUrlBeforeFetching('https://example.com/releases/noesora');
});

test('release asset checks reject loopback base URL overrides with credentials before fetching', async () => {
  await assertRejectedBaseUrlBeforeFetching('http://user:password@127.0.0.1:43129/release');
});

test('release asset checks reject loopback base URL overrides with a query before fetching', async () => {
  await assertRejectedBaseUrlBeforeFetching('http://127.0.0.1:43129/release?token=fixture');
});

test('release asset checks reject loopback base URL overrides with a fragment before fetching', async () => {
  await assertRejectedBaseUrlBeforeFetching('http://127.0.0.1:43129/release#fixture');
});

test('V61 expected manifest rejects downloaded bytes that differ despite a matching remote manifest', async () => {
  const expectedBytes = Buffer.from('original retained CI bytes');
  const differentBytes = Buffer.from('different downloaded bytes');
  const expectedManifest = manifestFor(expectedBytes);
  await assert.rejects(checkReleaseAssets({
    baseUrl: 'http://127.0.0.1:43129/release',
    expectedManifest,
    fetchImpl: async (url) => String(url).endsWith('/SHA256SUMS')
      ? textResponse(expectedManifest)
      : new Response(differentBytes),
  }), /SHA-256 mismatch/);
});

test('V62 creation is offered only with complete retained artifacts and read-only metadata', async () => {
  await withOriginalArtifacts(async ({ manifestPath, manifest, assets }) => {
    const env = releaseEnv();
    const requests = [];
    const result = await checkPublication({
      manifestPath,
      env,
      fetchImpl: publicationFetch({ env, manifest, assets, requests }),
    });
    assert.equal(result, 'create');
    assert(requests.length >= 3);
    assert(requests.every((request) => request.method === 'GET'));
    assert(requests.every(({ url }) => !url.includes('/releases/download/')));
  });
});

test('V62 publication refuses existing or unknown npm version before release lookup', async () => {
  await withOriginalArtifacts(async ({ manifestPath, manifest, assets }) => {
    const env = releaseEnv();
    for (const status of [200, 403]) {
      const requests = [];
      await assert.rejects(checkPublication({
        manifestPath,
        env,
        fetchImpl: publicationFetch({ env, manifest, assets, registryStatus: status, requests }),
      }));
      assert.equal(requests.length, 2);
      assert(requests.every((request) => request.method === 'GET'));
      assert(requests.every(({ url }) => !url.includes('/releases/tags/')));
    }
  });
});

test('V62 original publish-job retry reuses only the exact same-run release read-only', async () => {
  await withOriginalArtifacts(async ({ manifestPath, manifest, assets }) => {
    const env = releaseEnv();
    const requests = [];
    const result = await checkPublication({
      manifestPath,
      env,
      fetchImpl: publicationFetch({
        env,
        manifest,
        assets,
        releaseStatus: 200,
        release: existingRelease(env),
        requests,
      }),
    });
    assert.equal(result, 'reuse');
    assert.equal(requests.filter(({ url }) => url.endsWith('/SHA256SUMS')).length, 1);
    assert.equal(requests.filter(({ url }) => url.includes('/releases/download/')).length, 6);
    assert(requests.every((request) => request.method === 'GET'));
  });
});

test('V62 refuses new-dispatch and other-run release recovery', async () => {
  await withOriginalArtifacts(async ({ manifestPath, manifest, assets }) => {
    const firstAttempt = releaseEnv({ GITHUB_RUN_ATTEMPT: '1' });
    await assert.rejects(checkPublication({
      manifestPath,
      env: firstAttempt,
      fetchImpl: publicationFetch({
        env: firstAttempt,
        manifest,
        assets,
        releaseStatus: 200,
        release: existingRelease(firstAttempt),
      }),
    }), /new publish-job dispatch/);

    const current = releaseEnv();
    const otherRun = releaseEnv({ GITHUB_RUN_ID: '987654321' });
    await assert.rejects(checkPublication({
      manifestPath,
      env: current,
      fetchImpl: publicationFetch({
        env: current,
        manifest,
        assets,
        releaseStatus: 200,
        release: existingRelease(current, { body: expectedMarker(otherRun) }),
      }),
    }), /exact current-run marker/);

    await assert.rejects(checkPublication({
      manifestPath,
      env: current,
      fetchImpl: publicationFetch({
        env: current,
        manifest,
        assets,
        releaseStatus: 200,
        release: existingRelease(current, { body: expectedMarker(releaseEnv({ ENGINE_SHA: 'e'.repeat(40) })) }),
      }),
    }), /exact current-run marker/);

    const wrongCliMarker = expectedMarker(releaseEnv({ CLI_SHA: 'f'.repeat(40) }));
    await assert.rejects(checkPublication({
      manifestPath,
      env: current,
      fetchImpl: publicationFetch({
        env: current,
        manifest,
        assets,
        releaseStatus: 200,
        release: existingRelease(current, { body: wrongCliMarker }),
      }),
    }), /exact current-run marker/);
  });
});

test('V62 refuses incomplete release metadata and missing original artifact evidence', async () => {
  await withOriginalArtifacts(async ({ directory, manifestPath, manifest, assets }) => {
    const env = releaseEnv();
    for (const release of [
      existingRelease(env, { draft: true }),
      existingRelease(env, { tag_name: 'v9.9.9' }),
      existingRelease(env, { assets: releaseAssetMetadata().slice(1) }),
      existingRelease(env, { assets: [...releaseAssetMetadata(), { name: 'unexpected' }] }),
    ]) {
      await assert.rejects(checkPublication({
        manifestPath,
        env,
        fetchImpl: publicationFetch({ env, manifest, assets, releaseStatus: 200, release }),
      }));
    }

    const requests = [];
    await rm(path.join(directory, assetNames[0]));
    await assert.rejects(checkPublication({
      manifestPath,
      env,
      fetchImpl: publicationFetch({ env, manifest, assets, requests }),
    }), /Original retained CI asset is unavailable/);
    assert.equal(requests.length, 0);
  });
});

test('V61 refuses retained CI bytes that disagree with their original manifest', async () => {
  await withOriginalArtifacts(async ({ directory, manifestPath, manifest, assets }) => {
    await writeFile(path.join(directory, assetNames[0]), 'different retained fixture bytes');
    const requests = [];
    await assert.rejects(checkPublication({
      manifestPath,
      env: releaseEnv(),
      fetchImpl: publicationFetch({ manifest, assets, requests }),
    }));
    assert.equal(requests.length, 0, 'Corrupt retained artifacts must stop before remote metadata reads');
  });
});

test('V61 existing-release recovery rejects self-consistent remote bytes that differ from CI', async () => {
  await withOriginalArtifacts(async ({ manifestPath, manifest, assets }) => {
    const env = releaseEnv();
    const remoteAssets = new Map(assetNames.map((name) => [name, Buffer.from(`different remote artifact: ${name}`)]));
    const requests = [];
    await assert.rejects(checkPublication({
      manifestPath,
      env,
      fetchImpl: publicationFetch({
        env,
        manifest: manifestForAssets(remoteAssets),
        assets: remoteAssets,
        releaseStatus: 200,
        release: existingRelease(env),
        requests,
      }),
    }), /differs from original CI artifacts/);
    assert(requests.every((request) => request.method === 'GET'));
    assert.equal(requests.filter(({ url }) => url.includes('/releases/download/')).length, 1);
    assert.notEqual(manifest, manifestForAssets(remoteAssets));
  });
});

test('Windows preserves an in-use executable and removes rejected staging bytes', {
  skip: process.platform === 'win32' ? false : 'requires Windows executable replacement semantics',
}, async () => {
  const packageRoot = await mkdtemp(path.join(os.tmpdir(), 'noesora-in-use-binary-'));
  let child;
  try {
    const directory = path.join(packageRoot, 'vendor', 'win32-x64');
    const binaryPath = path.join(directory, 'noesora.exe');
    await mkdir(directory, { recursive: true });
    await writeFile(path.join(packageRoot, 'package.json'), JSON.stringify({ version: packageManifest.version }));
    await copyFile(process.execPath, binaryPath);
    const originalHash = sha256(await readFile(binaryPath));

    child = spawn(binaryPath, ['-e', "process.stdout.write('ready\\n'); setInterval(() => {}, 1000);"], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    await waitForReadyChild(child);
    assert.equal(child.exitCode, null);

    const replacementBytes = Buffer.from('synthetic replacement binary');
    const assets = new Map(assetNames.map((name) => [name, replacementBytes]));
    const requests = [];
    await assert.rejects(installBinary({
      packageRoot,
      platform: 'win32',
      arch: 'x64',
      baseUrl: 'http://127.0.0.1:43129/release',
      fetchImpl: async (input) => {
        const url = String(input);
        requests.push(url);
        return url.endsWith('/SHA256SUMS')
          ? textResponse(manifestForAssets(assets))
          : new Response(replacementBytes);
      },
    }));
    assert.deepEqual(requests, [
      'http://127.0.0.1:43129/release/SHA256SUMS',
      'http://127.0.0.1:43129/release/noesora-win32-x64.exe',
    ]);
    assert.equal(sha256(await readFile(binaryPath)), originalHash);
    assert.deepEqual(await readdir(directory), ['noesora.exe']);
  } finally {
    try {
      if (child) await stopChild(child);
    } finally {
      await rm(packageRoot, { recursive: true, force: true });
    }
  }
});
