import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { access, mkdtemp, readFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { checkReleaseAssets } from './check-release-assets.mjs';
import { targetFor } from './install-binary.mjs';

const [packageTarballArg, binaryArg] = process.argv.slice(2);
assert(packageTarballArg && binaryArg, 'Usage: node scripts/assert-npm-install.mjs <package-tarball> <native-binary>');
const packageTarball = path.resolve(packageTarballArg);
const nativeBinary = path.resolve(binaryArg);
const packageManifest = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'));
const targets = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-x64'];
const target = targetFor(process.platform, process.arch);
const assetNames = targets.map((item) => `noesora-${item}${item === 'win32-x64' ? '.exe' : ''}`);
const assetName = `noesora-${target}${process.platform === 'win32' ? '.exe' : ''}`;
const binaryName = process.platform === 'win32' ? 'noesora.exe' : 'noesora';
const bytes = await readFile(nativeBinary);
assert(bytes.length > 0, 'Release fixture binary must not be empty');
const digest = createHash('sha256').update(bytes).digest('hex');
assert.throws(() => targetFor('freebsd', 'x64'), /Unsupported target/);

const requests = [];
const checksumFor = (mode, name) => mode === 'bad' && name === assetName ? '0'.repeat(64) : digest;
const checksumManifest = (mode) => assetNames.map((name) => `${checksumFor(mode, name)}  ${name}`).join('\n') + '\n';
const server = createServer((request, response) => {
  const url = new URL(request.url || '/', 'http://127.0.0.1');
  requests.push({ method: request.method, path: url.pathname });
  const [mode, name] = url.pathname.split('/').filter(Boolean);
  if (!['good', 'bad', 'missing'].includes(mode)) {
    response.writeHead(404).end();
    return;
  }
  if (name === 'SHA256SUMS') {
    response.writeHead(200, { 'content-type': 'text/plain' }).end(checksumManifest(mode));
    return;
  }
  if (mode === 'missing' && request.method === 'HEAD' && name === 'noesora-linux-arm64') {
    response.writeHead(404).end();
    return;
  }
  if (assetNames.includes(name)) {
    if (request.method === 'HEAD') response.writeHead(200, { 'content-length': bytes.length }).end();
    else response.writeHead(200, { 'content-type': 'application/octet-stream' }).end(bytes);
    return;
  }
  response.writeHead(404).end();
});
await new Promise((resolve, reject) => {
  server.once('error', reject);
  server.listen(0, '127.0.0.1', resolve);
});
const address = server.address();
assert(address && typeof address === 'object');
const releaseBase = `http://127.0.0.1:${address.port}`;
const tempRoot = await mkdtemp(path.join(os.tmpdir(), 'noesora-npm-install-'));

function runNpmInstall(prefix, mode) {
  return runCommand('npm', [
    'install', '--prefix', prefix, '--no-save', '--no-audit', '--no-fund', '--ignore-scripts=false', packageTarball,
  ], { NOESORA_RELEASE_BASE_URL: `${releaseBase}/${mode}` }, process.platform === 'win32');
}

function runCommand(command, args, env, shell = false) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      shell,
      env: { ...process.env, ...env },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    child.stdout.setEncoding('utf8').on('data', (chunk) => { output += chunk; });
    child.stderr.setEncoding('utf8').on('data', (chunk) => { output += chunk; });
    child.once('error', reject);
    child.once('close', (status) => resolve({ status, output }));
  });
}

try {
  await checkReleaseAssets({ baseUrl: `${releaseBase}/good` });
  await assert.rejects(checkReleaseAssets({ baseUrl: `${releaseBase}/missing` }), /Release asset is unavailable/);

  const goodPrefix = path.join(tempRoot, 'good');
  const goodInstall = await runNpmInstall(goodPrefix, 'good');
  assert.equal(goodInstall.status, 0, goodInstall.output);

  const launcher = path.join(goodPrefix, 'node_modules', 'noesora', 'bin', 'noesora.js');
  const installedBinary = path.join(goodPrefix, 'node_modules', 'noesora', 'vendor', target, binaryName);
  const versionResult = spawnSync(process.execPath, [launcher, '--version'], { encoding: 'utf8' });
  assert.equal(versionResult.status, 0, versionResult.stderr || versionResult.error?.message);
  assert.equal(versionResult.stdout.trim(), `noesora ${packageManifest.version}`);
  const invalidResult = spawnSync(process.execPath, [launcher, '--noesora-test-invalid-option'], { encoding: 'utf8' });
  assert.equal(invalidResult.status, 2, invalidResult.stderr || 'Native CLI exit code must be preserved');
  assert.match(invalidResult.stderr, /error/i);
  if (process.platform !== 'win32') {
    const { mode } = await import('node:fs/promises').then(({ stat }) => stat(installedBinary));
    assert.notEqual(mode & 0o111, 0, 'Installed binary must be executable');
  }
  assert(requests.some(({ method, path: requestPath }) => method === 'GET' && requestPath === `/good/SHA256SUMS`));
  assert(requests.some(({ method, path: requestPath }) => method === 'GET' && requestPath === `/good/${assetName}`));

  const badPrefix = path.join(tempRoot, 'bad');
  const badInstall = await runNpmInstall(badPrefix, 'bad');
  assert.notEqual(badInstall.status, 0, 'Install must reject a checksum mismatch');
  assert.match(badInstall.output, /SHA-256 mismatch/i);
  await assert.rejects(access(path.join(badPrefix, 'node_modules', 'noesora', 'vendor', target, binaryName)), { code: 'ENOENT' });
  console.log(`npm install smoke passed for ${target}; checksum mismatch and missing release assets were rejected.`);
} finally {
  await new Promise((resolve) => server.close(resolve));
  await rm(tempRoot, { recursive: true, force: true });
}
