import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { after, before, test } from 'node:test';
import { createServer } from 'node:http';
import { chmod, mkdtemp, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { installBinary, targetFor } from '../scripts/install-binary.mjs';

const assetBytes = Buffer.from('deterministic native binary fixture\0');
const assetDigest = createHash('sha256').update(assetBytes).digest('hex');
const fixturePlatform = process.platform === 'win32' ? 'win32' : 'linux';
const fixtureArch = process.arch === 'arm64' ? 'arm64' : 'x64';
const fixtureTarget = `${fixturePlatform}-${fixtureArch}`;
const assetName = `noesora-${fixtureTarget}${fixturePlatform === 'win32' ? '.exe' : ''}`;
const binaryName = fixturePlatform === 'win32' ? 'noesora.exe' : 'noesora';
const targets = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64'];
let releaseBase;

function checksumManifest(mode) {
  return targets
    .filter((target) => !(mode === 'missing-checksum' && target === fixtureTarget))
    .map((target) => {
      const name = `noesora-${target}${target === 'win32-x64' ? '.exe' : ''}`;
      const digest = mode === 'wrong-hash' && target === fixtureTarget ? '0'.repeat(64) : assetDigest;
      return `${digest}  ${name}`;
    })
    .join('\n') + '\n';
}

const server = createServer((request, response) => {
  const url = new URL(request.url || '/', 'http://127.0.0.1');
  const [mode, name] = url.pathname.split('/').filter(Boolean);
  if (request.method !== 'GET') {
    response.writeHead(405).end();
    return;
  }
  if (name === 'SHA256SUMS') {
    if (mode === 'checksum-unavailable') {
      response.writeHead(503).end();
      return;
    }
    response.writeHead(200, { 'content-type': 'text/plain' }).end(checksumManifest(mode));
    return;
  }
  if (name === assetName) {
    if (mode === 'missing-asset') {
      response.writeHead(404).end();
      return;
    }
    if (mode === 'empty-asset') {
      response.writeHead(200, { 'content-type': 'application/octet-stream' }).end();
      return;
    }
    response.writeHead(200, { 'content-type': 'application/octet-stream' }).end(assetBytes);
    return;
  }
  response.writeHead(404).end();
});

before(async () => {
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  assert(address && typeof address === 'object');
  releaseBase = `http://127.0.0.1:${address.port}`;
});

after(async () => {
  if (server.listening) await new Promise((resolve) => server.close(resolve));
});

async function withPackageRoot(run) {
  const packageRoot = await mkdtemp(path.join(os.tmpdir(), 'noesora-installer-test-'));
  try {
    await writeFile(path.join(packageRoot, 'package.json'), JSON.stringify({ version: '0.1.0' }));
    return await run(packageRoot);
  } finally {
    await rm(packageRoot, { recursive: true, force: true });
  }
}

async function seedExistingBinary(packageRoot) {
  const directory = path.join(packageRoot, 'vendor', fixtureTarget);
  const binaryPath = path.join(directory, binaryName);
  await mkdir(directory, { recursive: true });
  await writeFile(binaryPath, 'previous-good-binary');
  if (fixturePlatform !== 'win32') await chmod(binaryPath, 0o755);
  return { directory, binaryPath };
}

async function installMode(packageRoot, mode, options = {}) {
  return installBinary({
    packageRoot,
    platform: fixturePlatform,
    arch: fixtureArch,
    baseUrl: `${releaseBase}/${mode}`,
    ...options,
  });
}

test('supported targets are explicit and unsupported hosts fail', () => {
  assert.equal(targetFor('darwin', 'arm64'), 'darwin-arm64');
  assert.equal(targetFor('linux', 'x64'), 'linux-x64');
  assert.equal(targetFor('win32', 'x64'), 'win32-x64');
  assert.throws(() => targetFor('freebsd', 'x64'), /Unsupported target/);
});

test('verified bytes atomically replace an existing binary', async () => {
  await withPackageRoot(async (packageRoot) => {
    const { binaryPath } = await seedExistingBinary(packageRoot);
    await installMode(packageRoot, 'good');
    assert.deepEqual(await readFile(binaryPath), assetBytes);
    if (fixturePlatform !== 'win32' && process.platform !== 'win32') {
      assert.notEqual((await stat(binaryPath)).mode & 0o111, 0);
    }
  });
});

for (const [mode, message] of [
  ['checksum-unavailable', /Checksum manifest download failed with HTTP 503/],
  ['missing-checksum', /Checksum manifest has no entry/],
  ['wrong-hash', /SHA-256 mismatch/],
  ['empty-asset', /Downloaded binary is empty/],
  ['missing-asset', /Binary download failed with HTTP 404/],
]) {
  test(`${mode} leaves the previous binary intact`, async () => {
    await withPackageRoot(async (packageRoot) => {
      const { directory, binaryPath } = await seedExistingBinary(packageRoot);
      await assert.rejects(installMode(packageRoot, mode), message);
      assert.equal(await readFile(binaryPath, 'utf8'), 'previous-good-binary');
      assert.deepEqual(await readdir(directory), [binaryName]);
    });
  });
}

test('a failed replacement leaves the previous binary intact and removes staging bytes', async () => {
  await withPackageRoot(async (packageRoot) => {
    const { directory, binaryPath } = await seedExistingBinary(packageRoot);
    const replacementError = new Error('simulated replacement failure');
    await assert.rejects(
      installMode(packageRoot, 'good', { renameImpl: async () => { throw replacementError; } }),
      (error) => error === replacementError,
    );
    assert.equal(await readFile(binaryPath, 'utf8'), 'previous-good-binary');
    assert.deepEqual(await readdir(directory), [binaryName]);
  });
});

test('unsupported target fails before it can replace a binary', async () => {
  await withPackageRoot(async (packageRoot) => {
    const { binaryPath } = await seedExistingBinary(packageRoot);
    await assert.rejects(
      installBinary({
        packageRoot,
        platform: 'freebsd',
        arch: 'x64',
        baseUrl: `${releaseBase}/good`,
      }),
      /Unsupported target/,
    );
    assert.equal(await readFile(binaryPath, 'utf8'), 'previous-good-binary');
  });
});
